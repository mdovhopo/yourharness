import AppKit
import HarnessCore

enum HarnessError: LocalizedError {
    case claudeNotFound
    case projectMissing

    var errorDescription: String? {
        switch self {
        case .claudeNotFound:
            "Couldn't find a working `claude` CLI. Check Settings → Agents."
        case .projectMissing:
            "This thread's project no longer exists."
        }
    }
}

@MainActor @Observable
final class AppModel {
    static let shared = AppModel()

    private(set) var projects: [Project] = []
    private(set) var threads: [ThreadModel] = []
    var selectedThreadID: UUID? {
        didSet { save() }
    }
    /// Last usage report from the CLI. Persisted, so limits show right after launch.
    private(set) var rateLimit: RateLimitInfo?
    private(set) var rateLimitUpdatedAt: Date?
    private(set) var environmentReady = false
    private(set) var homeProjectID: UUID
    /// Discovery results keyed by `AgentCLI.id`.
    private(set) var cliStatuses: [String: CLIStatus] = [:]
    private(set) var claudeAccount: ClaudeAccount?
    private(set) var isRefreshingAgents = false
    private(set) var cliPathOverrides: [String: String]
    private(set) var defaults: ThreadDefaults
    private(set) var claudeModels: [ClaudeModel]
    @ObservationIgnored private var contextWindows: [String: Int]

    @ObservationIgnored let store = Store()
    @ObservationIgnored private var environment: [String: String] = [:]
    @ObservationIgnored private var bootstrap: Task<Void, Never>?

    private init() {
        let state = store.loadState()
        var projects = state.projects
        // Threads can always start somewhere, even before you add a project.
        if let id = state.homeProjectID, projects.contains(where: { $0.id == id }) {
            homeProjectID = id
        } else {
            let home = Project(name: "Home", path: NSHomeDirectory())
            projects.insert(home, at: 0)
            homeProjectID = home.id
        }
        self.projects = projects
        cliPathOverrides = state.cliPathOverrides
        defaults = state.defaults
        claudeModels = state.claudeModels
        contextWindows = state.contextWindows
        rateLimit = state.rateLimit
        rateLimitUpdatedAt = state.rateLimitUpdatedAt

        threads = state.threads.map { ThreadModel(meta: $0, app: self) }
        selectedThreadID = state.selectedThreadID
        if selectedThread == nil { newThreadInCurrentProject() }
        bootstrap = Task {
            await loadEnvironment()
            await refreshAgents()
        }
    }

    var homeProject: Project {
        project(id: homeProjectID) ?? projects[0]
    }

    func isHome(_ project: Project) -> Bool {
        project.id == homeProjectID
    }

    var claudeExecutable: URL? {
        cliStatuses[AgentCLI.claude.id]?.path
    }

    var selectedThread: ThreadModel? {
        threads.first { $0.id == selectedThreadID }
    }

    var selectedProject: Project {
        selectedThread.flatMap { project(id: $0.projectID) } ?? homeProject
    }

    func project(id: UUID) -> Project? {
        projects.first { $0.id == id }
    }

    func threads(in project: Project) -> [ThreadModel] {
        threads.filter { $0.projectID == project.id }.sorted { $0.updatedAt > $1.updatedAt }
    }

    // MARK: - Projects

    func presentAddProject() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.prompt = "Add Project"
        panel.message = "Choose a project folder. Claude runs with this folder as its working directory."
        guard panel.runModal() == .OK, let url = panel.url else { return }
        addProject(at: url)
    }

    func addProject(at url: URL) {
        if let existing = projects.first(where: { $0.path == url.path }) {
            newThread(in: existing)
            return
        }
        let project = Project(name: url.lastPathComponent, path: url.path,
                              permissionMode: defaults.permissionMode ?? .ask)
        projects.append(project)
        newThread(in: project)
    }

    func removeProject(_ project: Project) {
        guard !isHome(project) else { return }
        for thread in threads(in: project) { deleteThread(thread) }
        projects.removeAll { $0.id == project.id }
        save()
    }

    /// Points the Home project at another folder (e.g. `~/dev`).
    func setHomeFolder(_ url: URL?) {
        guard let index = projects.firstIndex(where: { $0.id == homeProjectID }) else { return }
        let url = url ?? URL(fileURLWithPath: NSHomeDirectory())
        projects[index].path = url.path
        projects[index].name = url.path == NSHomeDirectory() ? "Home" : url.lastPathComponent
        save()
        for thread in threads where thread.projectID == homeProjectID { thread.restartSession() }
    }

    func presentChooseHomeFolder() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.directoryURL = homeProject.url
        panel.prompt = "Use Folder"
        panel.message = "New threads that aren't in a project start in this folder."
        guard panel.runModal() == .OK, let url = panel.url else { return }
        setHomeFolder(url)
    }

    func setPermissionMode(_ mode: PermissionMode, for projectID: UUID) {
        guard let index = projects.firstIndex(where: { $0.id == projectID }) else { return }
        projects[index].permissionMode = mode
        save()
        for thread in threads where thread.projectID == projectID { thread.applyPermissionMode(mode) }
    }

    func revealInFinder(_ project: Project) {
        NSWorkspace.shared.activateFileViewerSelecting([project.url])
    }

    // MARK: - Threads

    @discardableResult
    func newThread(in project: Project) -> ThreadModel {
        // Reuse an untouched empty thread instead of piling up blanks.
        if let blank = threads(in: project).first(where: { $0.entries.isEmpty && $0.sessionID == nil && $0.title == ThreadModel.untitled }) {
            selectedThreadID = blank.id
            return blank
        }
        let thread = ThreadModel(projectID: project.id, app: self)
        threads.append(thread)
        selectedThreadID = thread.id
        return thread
    }

    func newThreadInCurrentProject() {
        newThread(in: selectedProject)
    }

    func deleteThread(_ thread: ThreadModel) {
        thread.endSession()
        threads.removeAll { $0.id == thread.id }
        store.deleteTranscript(thread.id)
        if selectedThreadID == thread.id { selectedThreadID = nil }
        save()
    }

    // MARK: - Launching

    func launchOptions(for thread: ThreadModel) async throws -> ClaudeLaunchOptions {
        await bootstrap?.value
        guard let executable = claudeExecutable else { throw HarnessError.claudeNotFound }
        guard let project = project(id: thread.projectID) else { throw HarnessError.projectMissing }
        return ClaudeLaunchOptions(
            executable: executable,
            workingDirectory: project.url,
            environment: environment,
            model: thread.effectiveModel,
            permissionMode: project.permissionMode.cliValue,
            effort: thread.effectiveEffort,
            resumeSessionID: thread.sessionID
        )
    }

    // MARK: - Models & defaults

    /// The catalog entry for a `--model` value; nil value means the CLI default.
    func claudeModel(for value: String?) -> ClaudeModel? {
        claudeModels.first { $0.value == (value ?? "default") }
    }

    /// "Opus 5.5" rather than "Default (recommended)", so you always see what's running.
    func modelDisplayName(for value: String?) -> String {
        guard let model = claudeModel(for: value) else { return value?.capitalized ?? "Claude" }
        if model.value == "default",
           let concrete = claudeModels.first(where: { $0.value != "default" && $0.resolvedModel == model.resolvedModel }) {
            return concrete.displayName
        }
        return model.displayName
    }

    func effortLevels(for modelValue: String?) -> [EffortLevel] {
        let supported = claudeModel(for: modelValue)?.supportedEffortLevels ?? EffortLevel.allCases.map(\.rawValue)
        return EffortLevel.allCases.filter { supported.contains($0.rawValue) }
    }

    func updateCatalog(_ catalog: ClaudeCatalog) {
        guard !catalog.models.isEmpty, catalog.models != claudeModels else { return }
        claudeModels = catalog.models
        save()
    }

    func updateRateLimit(_ info: RateLimitInfo) {
        rateLimit = info
        rateLimitUpdatedAt = .now
        save()
    }

    func contextWindow(forModel model: String?) -> Int? {
        model.flatMap { contextWindows[$0] }
    }

    func rememberContextWindows(_ windows: [String: Int]) {
        let merged = contextWindows.merging(windows) { $1 }
        guard merged != contextWindows else { return }
        contextWindows = merged
        save()
    }

    func setDefaultModel(_ model: String?) {
        defaults.model = model
        save()
        for thread in threads where thread.model == nil { thread.applyModel() }
    }

    func setDefaultEffort(_ effort: String?) {
        defaults.effort = effort
        save()
        for thread in threads where thread.effort == nil { thread.restartSession() }
    }

    func setDefaultPermissionMode(_ mode: PermissionMode?) {
        defaults.permissionMode = mode
        save()
    }

    private func loadEnvironment() async {
        var environment = await ShellEnvironment.load()
        // Inherited when launched from a Claude Code terminal; would confuse the child CLI.
        for key in ["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_SSE_PORT"] {
            environment.removeValue(forKey: key)
        }
        self.environment = environment
        environmentReady = true
    }

    // MARK: - Agents

    /// Re-detects every known CLI in parallel and reloads the Claude account.
    func refreshAgents() async {
        guard environmentReady, !isRefreshingAgents else { return }
        isRefreshingAgents = true
        defer { isRefreshingAgents = false }

        let environment = environment
        let overrides = cliPathOverrides.mapValues { URL(fileURLWithPath: $0) }
        let statuses = await withTaskGroup(of: (String, CLIStatus).self) { group in
            for cli in AgentCLI.all {
                group.addTask {
                    (cli.id, await CLIDiscovery.probe(cli, environment: environment, override: overrides[cli.id]))
                }
            }
            var statuses: [String: CLIStatus] = [:]
            for await (id, status) in group { statuses[id] = status }
            return statuses
        }
        cliStatuses = statuses

        if let claude = statuses[AgentCLI.claude.id], claude.isReady, let path = claude.path {
            async let account = ClaudeAccount.load(executable: path, environment: environment)
            async let catalog = ClaudeCatalog.load(executable: path, environment: environment)
            claudeAccount = await account
            if let catalog = await catalog { updateCatalog(catalog) }
        } else {
            claudeAccount = nil
        }
    }

    func presentChooseExecutable(for cli: AgentCLI) {
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = false
        panel.treatsFilePackagesAsDirectories = true
        panel.showsHiddenFiles = true
        panel.prompt = "Use This \(cli.name)"
        panel.message = "Choose the \(cli.executableNames[0]) executable."
        guard panel.runModal() == .OK, let url = panel.url else { return }
        setExecutableOverride(url, for: cli)
    }

    func setExecutableOverride(_ url: URL?, for cli: AgentCLI) {
        cliPathOverrides[cli.id] = url?.path
        save()
        if cli == .claude {
            for thread in threads { thread.restartSession() }
        }
        Task { await refreshAgents() }
    }

    // MARK: - Persistence

    func save() {
        var state = PersistedState()
        state.projects = projects
        state.threads = threads.filter { !$0.isEphemeral }.map(\.meta)
        state.selectedThreadID = selectedThreadID
        state.homeProjectID = homeProjectID
        state.cliPathOverrides = cliPathOverrides
        state.defaults = defaults
        state.claudeModels = claudeModels
        state.contextWindows = contextWindows
        state.rateLimit = rateLimit
        state.rateLimitUpdatedAt = rateLimitUpdatedAt
        store.saveState(state)
    }

    func shutdown() {
        save()
        for thread in threads { thread.endSession() }
    }

    // MARK: - Debug

    func openStressThread() {
        let thread = ThreadModel(projectID: homeProjectID, app: self, isEphemeral: true)
        thread.title = "Stress Test · 10,000 entries"
        thread.seed(StressTest.entries(count: 10_000))
        threads.append(thread)
        selectedThreadID = thread.id
    }
}
