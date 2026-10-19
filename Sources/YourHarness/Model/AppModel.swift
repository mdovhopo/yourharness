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
        didSet {
            overviewProjectID = nil
            selectedThread?.markRead()
            save()
        }
    }
    /// When set, the detail pane shows this project's repos and PRs instead of a thread.
    private(set) var overviewProjectID: UUID?
    private(set) var collapsedProjectIDs: Set<UUID> = []
    /// Last usage report from the CLI. Persisted, so limits show right after launch.
    private(set) var rateLimit: RateLimitInfo?
    private(set) var rateLimitUpdatedAt: Date?
    private(set) var isRefreshingLimits = false
    /// Why the last on-demand refresh didn't produce a reading.
    private(set) var limitsRefreshError: String?
    private(set) var environmentReady = false
    private(set) var homeProjectID: UUID
    /// Discovery results keyed by `AgentCLI.id`.
    private(set) var cliStatuses: [String: CLIStatus] = [:]
    private(set) var claudeAccount: ClaudeAccount?
    /// `git`, `gh` and `glab`, keyed by executable name.
    private(set) var toolStatuses: [String: CLIStatus] = [:]
    private(set) var isRefreshingAgents = false
    private(set) var cliPathOverrides: [String: String]
    private(set) var defaults: ThreadDefaults
    private(set) var claudeModels: [ClaudeModel]
    /// Whether agents are told this app draws Mermaid diagrams.
    private(set) var diagramInstructions: Bool
    @ObservationIgnored private var contextWindows: [String: Int]

    @ObservationIgnored let store = Store()
    @ObservationIgnored private var environment: [String: String] = [:]
    @ObservationIgnored private var bootstrap: Task<Void, Never>?
    @ObservationIgnored private var workspaceModels: [UUID: WorkspaceModel] = [:]
    @ObservationIgnored private var workspacePoller: Task<Void, Never>?
    @ObservationIgnored private var activationObserver: NSObjectProtocol?

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
        collapsedProjectIDs = state.collapsedProjectIDs
        diagramInstructions = state.diagramInstructions

        threads = state.threads.map { ThreadModel(meta: $0, app: self) }
        selectedThreadID = state.selectedThreadID
        if selectedThread == nil { newThreadInCurrentProject() }
        if let id = state.overviewProjectID, let project = project(id: id), !isHome(project) {
            overviewProjectID = id
        }
        bootstrap = Task {
            await loadEnvironment()
            await refreshAgents()
        }
        Task {
            await bootstrap?.value
            refreshWorkspaces()
        }
        startWorkspacePolling()
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

    /// The thread on screen right now; nil while a project overview is showing.
    var visibleThreadID: UUID? {
        overviewProjectID == nil ? selectedThreadID : nil
    }

    var overviewProject: Project? {
        overviewProjectID.flatMap(project(id:))
    }

    func showOverview(of project: Project) {
        guard !isHome(project) else { return }
        overviewProjectID = project.id
        save()
        if let workspace = workspace(for: project) {
            let tools = workspaceTools
            Task { await workspace.refresh(tools: tools, pullsMaxAge: 60) }
        }
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

    /// Everything the sidebar shows, in order. Lets the list animate inserts and reorders.
    var sidebarOrder: [UUID] {
        projects.flatMap { project in
            [project.id] + (isCollapsed(project) ? [] : threads(in: project).map(\.id))
        }
    }

    /// Threads in sidebar order, skipping collapsed projects. For keyboard navigation.
    var visibleThreadIDs: [UUID] {
        projects.flatMap { isCollapsed($0) ? [] : threads(in: $0).map(\.id) }
    }

    func selectAdjacentThread(_ offset: Int) {
        let ids = visibleThreadIDs
        guard !ids.isEmpty else { return }
        guard let current = visibleThreadID, let index = ids.firstIndex(of: current) else {
            selectedThreadID = offset > 0 ? ids.first : ids.last
            return
        }
        selectedThreadID = ids[min(max(index + offset, 0), ids.count - 1)]
    }

    func isCollapsed(_ project: Project) -> Bool {
        collapsedProjectIDs.contains(project.id)
    }

    func setCollapsed(_ collapsed: Bool, for project: Project) {
        if collapsed {
            collapsedProjectIDs.insert(project.id)
        } else {
            collapsedProjectIDs.remove(project.id)
        }
        save()
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
                              permissionMode: defaults.permissionMode ?? .standard)
        projects.append(project)
        newThread(in: project)
        refreshWorkspaces()
    }

    func removeProject(_ project: Project) {
        guard !isHome(project) else { return }
        for thread in threads(in: project) { deleteThread(thread) }
        projects.removeAll { $0.id == project.id }
        workspaceModels[project.id] = nil
        if overviewProjectID == project.id { overviewProjectID = nil }
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
        if isCollapsed(project) { setCollapsed(false, for: project) }
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

    // MARK: - Workspaces

    /// Repos and PRs for a project. Home is never scanned: it's usually your whole home
    /// folder, not a workspace.
    func workspace(for project: Project) -> WorkspaceModel? {
        guard !isHome(project) else { return nil }
        if let existing = workspaceModels[project.id], existing.root == project.url { return existing }
        let model = WorkspaceModel(projectID: project.id, root: project.url)
        workspaceModels[project.id] = model
        return model
    }

    /// The account a forge CLI reported in any workspace refresh.
    func forgeAccount(for forge: Forge) -> String? {
        workspaceModels.values.lazy.compactMap { $0.forgeAccounts[forge] }.first
    }

    /// What `#12` in this thread's text can link to: the project's repos and open PRs.
    func issueLinker(for thread: ThreadModel) -> IssueLinker {
        guard let project = project(id: thread.projectID), let workspace = workspace(for: project) else {
            return IssueLinker(repos: [], openPullRequests: [])
        }
        let repos = workspace.clonedRepos.compactMap { repo in
            workspace.remotes[repo.id].map { IssueLinker.Repo(name: repo.url.lastPathComponent, remote: $0) }
        }
        return IssueLinker(repos: repos, openPullRequests: workspace.pullRequests)
    }

    var workspaceTools: WorkspaceTools {
        let ready = { (name: String) in self.toolStatuses[name].flatMap { $0.isReady ? $0.path : nil } }
        return WorkspaceTools(
            git: ready("git").map { Git(executable: $0, environment: environment) },
            github: ready("gh").map { GitHubClient(executable: $0, environment: environment) },
            gitlab: ready("glab").map { GitLabClient(executable: $0, environment: environment) }
        )
    }

    func refreshWorkspaces(rescan: Bool = false, pullsMaxAge: TimeInterval = 120) {
        guard environmentReady else { return }
        let tools = workspaceTools
        for project in projects {
            guard let workspace = workspace(for: project) else { continue }
            Task { await workspace.refresh(tools: tools, rescan: rescan, pullsMaxAge: pullsMaxAge) }
        }
    }

    /// An agent may have edited files, committed, or opened a PR.
    func turnFinished(in projectID: UUID) {
        guard let project = project(id: projectID), let workspace = workspace(for: project) else { return }
        let tools = workspaceTools
        Task {
            await workspace.refreshStatus(tools: tools, minInterval: 0)
            await workspace.refreshPulls(tools: tools, maxAge: 15)
        }
    }

    /// Refresh when you come back to the app, and every few minutes while it's frontmost.
    private func startWorkspacePolling() {
        activationObserver = NotificationCenter.default.addObserver(
            forName: NSApplication.didBecomeActiveNotification, object: nil, queue: .main
        ) { _ in
            MainActor.assumeIsolated { AppModel.shared.refreshWorkspaces() }
        }
        workspacePoller = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(300))
                guard let self, NSApp.isActive else { continue }
                self.refreshWorkspaces(pullsMaxAge: 240)
            }
        }
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
            resumeSessionID: thread.sessionID,
            appendSystemPrompt: diagramInstructions ? AgentInstructions.diagrams : nil
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

    /// Old cached catalogs don't say; assume yes until a fresh one arrives.
    func supportsAutoMode(_ modelValue: String?) -> Bool {
        claudeModel(for: modelValue)?.supportsAutoMode ?? true
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
        limitsRefreshError = nil
        save()
    }

    /// Asks Claude for a fresh usage reading with a tiny throwaway request.
    func refreshLimits() async {
        guard !isRefreshingLimits else { return }
        isRefreshingLimits = true
        defer { isRefreshingLimits = false }
        await bootstrap?.value
        guard let executable = claudeExecutable else {
            limitsRefreshError = HarnessError.claudeNotFound.errorDescription
            return
        }
        switch await ClaudeUsageProbe.run(executable: executable, environment: environment) {
        case .success(let info):
            updateRateLimit(info)
        case .failure(.timedOut):
            limitsRefreshError = "Claude didn't answer in time."
        case .failure(.noReport(let detail)):
            limitsRefreshError = detail
        }
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

    /// The instruction is part of the launch, so running sessions pick it up on restart.
    func setDiagramInstructions(_ enabled: Bool) {
        diagramInstructions = enabled
        save()
        for thread in threads { thread.restartSession() }
    }

    func setDefaultPermissionMode(_ mode: PermissionMode?) {
        defaults.permissionMode = mode
        save()
    }

    private func loadEnvironment() async {
        // Starts from a clean login shell, so a Claude Code session that launched this app
        // can't leak its session into the threads' `claude` processes.
        environment = await ShellEnvironment.load()
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

        let tools = await withTaskGroup(of: (String, CLIStatus).self) { group in
            for name in ["git", Forge.github.cliName, Forge.gitlab.cliName] {
                group.addTask { (name, await CLIDiscovery.probe(executableNames: [name], environment: environment)) }
            }
            var tools: [String: CLIStatus] = [:]
            for await (name, status) in group { tools[name] = status }
            return tools
        }
        toolStatuses = tools

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
        state.overviewProjectID = overviewProjectID
        state.homeProjectID = homeProjectID
        state.cliPathOverrides = cliPathOverrides
        state.defaults = defaults
        state.claudeModels = claudeModels
        state.contextWindows = contextWindows
        state.rateLimit = rateLimit
        state.rateLimitUpdatedAt = rateLimitUpdatedAt
        state.collapsedProjectIDs = collapsedProjectIDs
        state.diagramInstructions = diagramInstructions
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
