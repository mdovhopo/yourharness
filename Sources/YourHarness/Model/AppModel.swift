import AppKit
import HarnessCore
import SwiftUI

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
            if let selectedThreadID { notifier.clear(thread: selectedThreadID) }
            noteViewed()
            save()
        }
    }

    /// Threads you looked at, newest first. Beyond the last few, an idle thread's transcript
    /// is let go: a long one is tens of megabytes in memory, and reading it back is quick.
    @ObservationIgnored private var viewed: [UUID] = []
    private static let loadedTranscripts = 6

    private func noteViewed() {
        guard let selectedThreadID else { return }
        viewed = [selectedThreadID] + viewed.filter { $0 != selectedThreadID }
        guard viewed.count > Self.loadedTranscripts else { return }
        let stale = Set(viewed.dropFirst(Self.loadedTranscripts))
        for thread in threads where stale.contains(thread.id) { thread.unloadTranscript() }
        viewed = Array(viewed.prefix(Self.loadedTranscripts))
    }
    /// When set, the detail pane shows this project's page instead of a thread: its repos and
    /// PRs, or its memory (`projectTab`).
    private(set) var overviewProjectID: UUID?

    /// The project page's two halves.
    enum ProjectTab: String, CaseIterable, Identifiable {
        case workspace, memory
        var id: String { rawValue }
        var label: String {
            switch self {
            case .workspace: "Pull Requests"
            case .memory: "Memory"
            }
        }
        var symbol: String {
            switch self {
            case .workspace: "arrow.triangle.pull"
            case .memory: "brain"
            }
        }
    }

    var projectTab = ProjectTab.workspace
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
    private(set) var titleGeneration: TitleGenerationSettings
    /// Why the last generated title didn't arrive, for Settings; cleared by the next success.
    private(set) var titleGenerationError: String?
    @ObservationIgnored private var contextWindows: [String: Int]

    /// Whether to notify you when a turn finishes or Claude needs you.
    private(set) var notifications: Bool
    /// Whether Claude gets its task tools, which the plan checklist shows.
    private(set) var planChecklist: Bool
    /// Per repo path: which threads have worked in it, for sorting repos by use.
    private(set) var repoUsage: [String: RepoUsage]
    /// Repos whose worktrees are shown in the repos view; folded is the default.
    private(set) var expandedWorktrees: Set<String>
    /// Whether Claude gets the browser built into the app.
    private(set) var browserTools: Bool
    private(set) var browserAutoShow: Bool
    @ObservationIgnored let notifier = Notifier()

    // Memory (see AppModel+Memory.swift).
    /// Whether agents get the workspace's memory.
    var memoryEnabled: Bool
    /// While memory is on, Claude Code's own memory files are off.
    var claudeCodeMemoryOff: Bool
    /// What the Importer and Reconciler run on, unless picked in their window; nil: the threads' default.
    var memoryAgentModel: String?
    var memoryAgentEffort: String?
    /// Removed projects' IDs by folder, so adding one back brings back its memory.
    @ObservationIgnored var removedProjectIDs: [String: UUID] = [:]
    /// Bumped whenever a scope's memory changes, so what shows it reloads.
    var memoryRevisions: [String: Int] = [:]
    /// The importer and reconciler conversations, by project and role: they outlive their window.
    var memoryAgents: [String: ThreadModel] = [:]
    @ObservationIgnored var memoryLibraries: [UUID: MemoryLibrary] = [:]
    /// ⌘K.
    @ObservationIgnored let spotlight = Spotlight()
    /// What the last direct import did, until dismissed.
    var importReport: MemoryImportReport?

    /// Each project's files for `@` mentions, and when they were listed.
    @ObservationIgnored private var fileIndexes: [UUID: (index: FileIndex, listedAt: Date)] = [:]
    @ObservationIgnored private var fileListings: [UUID: Task<FileIndex, Never>] = [:]

    @ObservationIgnored let store = Store()
    @ObservationIgnored let updater = Updater()
    @ObservationIgnored private(set) var environment: [String: String] = [:]
    @ObservationIgnored private(set) var bootstrap: Task<Void, Never>?
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
            // A development run gets its own folder as Home: running `claude` in your real
            // home folder would set off macOS's prompts for Desktop, Documents and the rest.
            let home = Project(name: "Home", path: store.isScratch ? store.root.appending(path: "home").path : NSHomeDirectory())
            if store.isScratch { try? FileManager.default.createDirectory(at: store.root.appending(path: "home"), withIntermediateDirectories: true) }
            projects.insert(home, at: 0)
            homeProjectID = home.id
        }
        self.projects = projects
        cliPathOverrides = state.cliPathOverrides
        defaults = state.defaults
        claudeModels = state.claudeModels
        slashCommands = Dictionary(uniqueKeysWithValues: state.slashCommands.compactMap { key, commands in
            UUID(uuidString: key).map { ($0, commands) }
        })
        skillNames = Dictionary(uniqueKeysWithValues: state.skills.compactMap { key, skills in
            UUID(uuidString: key).map { ($0, skills) }
        })
        contextWindows = state.contextWindows
        rateLimit = state.rateLimit
        rateLimitUpdatedAt = state.rateLimitUpdatedAt
        collapsedProjectIDs = state.collapsedProjectIDs
        diagramInstructions = state.diagramInstructions
        autoSettleDays = state.autoSettleDays
        titleGeneration = state.titleGeneration
        showsInspector = state.showsInspector
        appearance = state.appearance
        notifications = state.notifications
        planChecklist = state.planChecklist
        browserTools = state.browserTools
        repoUsage = state.repoUsage
        expandedWorktrees = state.expandedWorktrees
        browserAutoShow = state.browserAutoShow
        memoryEnabled = state.memoryEnabled
        claudeCodeMemoryOff = state.claudeCodeMemoryOff
        removedProjectIDs = state.removedProjectIDs
        memoryAgentModel = state.memoryAgentModel
        memoryAgentEffort = state.memoryAgentEffort

        threads = state.threads.map { ThreadModel(meta: $0, app: self) }
        selectedThreadID = state.selectedThreadID
        if selectedThread == nil { newThreadInCurrentProject() }
        if let id = state.memoryProjectID, project(id: id) != nil {
            overviewProjectID = id
            projectTab = .memory
        } else if let id = state.overviewProjectID, let project = project(id: id), !isHome(project) {
            overviewProjectID = id
        }
        bootstrap = Task {
            await loadEnvironment()
            await refreshAgents()
        }
        Task {
            await bootstrap?.value
            refreshWorkspaces()
            settleIdleThreads()
            // Give the workspaces a moment to list branch PRs, then look for merged work.
            try? await Task.sleep(for: .seconds(20))
            await syncPullRequestsForSettling()
        }
        startWorkspacePolling()
        notifier.open = { [weak self] id in self?.openFromNotification(id) }
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

    private(set) var appearance: Appearance

    func setAppearance(_ appearance: Appearance) {
        guard appearance != self.appearance else { return }
        self.appearance = appearance
        save()
        applyAppearance()
    }

    /// For the whole app, Settings included. `YH_APPEARANCE=light|dark` overrides it for a
    /// development run without saving.
    func applyAppearance() {
        let override = ProcessInfo.processInfo.environment["YH_APPEARANCE"].flatMap(Appearance.init(rawValue:))
        NSApp.appearance = (override ?? appearance).nsAppearance
    }

    /// The right sidebar: a thread's side chat and its sub-agents. Stays as you left it.
    private(set) var showsInspector = false

    enum InspectorPanel: String, CaseIterable, Identifiable {
        case pullRequests, sideChat, subagents, browser
        var id: String { rawValue }
        var label: String {
            switch self {
            case .pullRequests: "PRs"
            case .sideChat: "Side Chat"
            case .subagents: "Sub-agents"
            case .browser: "Browser"
            }
        }
        var symbol: String {
            switch self {
            case .pullRequests: "arrow.triangle.pull"
            case .sideChat: "bubble.left.and.text.bubble.right"
            case .subagents: "person.2"
            case .browser: "safari"
            }
        }
    }

    var inspectorPanel = InspectorPanel.sideChat

    /// You picked a tab since switching threads: PRs turning up don't take it over.
    @ObservationIgnored private var panelPicked = false

    /// A thread with PRs opens the sidebar on them; one without leaves the PRs tab.
    func choosePanel(for thread: ThreadModel?) {
        panelPicked = false
        guard let thread else { return }
        if !workedPullRequests(for: thread).isEmpty {
            inspectorPanel = .pullRequests
        } else if inspectorPanel == .pullRequests {
            inspectorPanel = .sideChat
        }
    }

    /// The thread on screen just got its first PRs (found a moment after it opened, or
    /// Claude just made one): show them, unless you've picked another tab.
    func pullRequestsAppeared(for thread: ThreadModel) {
        guard !panelPicked, !workedPullRequests(for: thread).isEmpty else { return }
        inspectorPanel = .pullRequests
    }

    /// From the tab picker: your choice holds until you switch threads.
    func pickPanel(_ panel: InspectorPanel) {
        panelPicked = true
        inspectorPanel = panel
    }

    /// A thread is showing, so the right sidebar has something to show.
    var inspectorHasThread: Bool { overviewProject == nil && selectedThread != nil }

    func showInspector(_ panel: InspectorPanel) {
        inspectorPanel = panel
        setShowsInspector(true)
    }

    func setShowsInspector(_ shows: Bool) {
        guard shows != showsInspector else { return }
        showsInspector = shows
        save()
    }

    /// Opens the right sidebar on one sub-agent of the thread on screen.
    func inspect(_ run: SubagentRun) {
        selectedThread?.inspectedSubagentID = run.id
        showInspector(.subagents)
    }

    /// The thread on screen right now; nil while a project overview is showing.
    var visibleThreadID: UUID? {
        overviewProjectID == nil ? selectedThreadID : nil
    }

    var overviewProject: Project? {
        overviewProjectID.flatMap(project(id:))
    }

    /// Opens a project's page: on its pull requests unless told otherwise. Home has no
    /// workspace, so its page is its memory.
    func showProject(_ project: Project, tab: ProjectTab? = nil) {
        overviewProjectID = project.id
        projectTab = isHome(project) ? .memory : tab ?? .workspace
        save()
        if projectTab == .workspace, let workspace = workspace(for: project) {
            let tools = workspaceTools
            Task { await workspace.refresh(tools: tools, pullsMaxAge: 60) }
        }
    }

    func showOverview(of project: Project) {
        guard !isHome(project) else { return }
        overviewProjectID = project.id
        projectTab = .workspace
        save()
        if let workspace = workspace(for: project) {
            let tools = workspaceTools
            Task { await workspace.refresh(tools: tools, pullsMaxAge: 60) }
        }
    }

    /// The project whose memory is on screen.
    var memoryProjectID: UUID? { projectTab == .memory ? overviewProjectID : nil }

    var memoryProject: Project? {
        memoryProjectID.flatMap(project(id:))
    }

    /// Shows a project's memory in the detail pane: its page, on the Memory tab.
    func showMemory(of project: Project) {
        showProject(project, tab: .memory)
    }

    var selectedProject: Project {
        selectedThread.flatMap { project(id: $0.projectID) } ?? homeProject
    }

    func project(id: UUID) -> Project? {
        projects.first { $0.id == id }
    }

    /// Active threads: pinned first, then most recent.
    func threads(in project: Project) -> [ThreadModel] {
        threads.filter { $0.projectID == project.id && !$0.isSettled }.sorted { a, b in
            a.isPinned != b.isPinned ? a.isPinned : a.sortDate > b.sortDate
        }
    }

    /// The Settled shelf, most recently settled first.
    func settledThreads(in project: Project) -> [ThreadModel] {
        threads.filter { $0.projectID == project.id && $0.isSettled }.sorted { ($0.settledAt ?? .distantPast) > ($1.settledAt ?? .distantPast) }
    }

    /// Projects whose Settled shelf is open in the sidebar.
    private(set) var expandedShelves: Set<UUID> = []

    func setShelfExpanded(_ expanded: Bool, for project: Project) {
        if expanded { expandedShelves.insert(project.id) } else { expandedShelves.remove(project.id) }
    }

    func setPinned(_ pinned: Bool, _ thread: ThreadModel) {
        thread.isPinned = pinned
        save()
    }

    /// Settling moves finished work to the shelf, drops its pin and frees its `claude`
    /// process; the conversation stays. Un-settling brings it back to the top and holds
    /// off automatic settling until there's new activity.
    func setSettled(_ settled: Bool, _ thread: ThreadModel) {
        if settled {
            guard !thread.isSettled else { return }
            // Move the selection to a neighbour first, so the detail pane doesn't go blank.
            if visibleThreadID == thread.id { selectNeighbour(of: thread) }
            thread.settledAt = .now
            thread.isPinned = false
            if !thread.isWorking {
                thread.endSession()
                thread.closeBrowser()
            }
        } else {
            guard thread.isSettled else { return }
            thread.settledAt = nil
            thread.reactivatedAt = .now
        }
        save()
    }

    private func selectNeighbour(of thread: ThreadModel) {
        let ids = visibleThreadIDs
        let index = ids.firstIndex(of: thread.id) ?? 0
        selectedThreadID = ids.indices.contains(index + 1) ? ids[index + 1] : (index > 0 ? ids[index - 1] : nil)
    }

    func setAutoSettle(_ enabled: Bool, _ thread: ThreadModel) {
        thread.autoSettle = enabled
        save()
    }

    private(set) var autoSettleDays: Int

    func setAutoSettleDays(_ days: Int) {
        autoSettleDays = days
        save()
        settleIdleThreads()
    }

    /// Settles threads with no activity for `autoSettleDays`, the way T3 Code does: never
    /// while working or waiting on you, never one you've opted out, and never one you
    /// un-settled until you've sent it something new.
    func settleIdleThreads(now: Date = .now) {
        guard autoSettleDays > 0 else { return }
        let cutoff = now.addingTimeInterval(-Double(autoSettleDays) * 86_400)
        var changed = false
        for thread in threads {
            guard !thread.isSettled, !thread.isEphemeral, thread.autoSettle, thread.reactivatedAt == nil,
                  !thread.isWorking, thread.pendingPermissions.isEmpty,
                  // Not the one on screen, and not an untouched blank.
                  visibleThreadID != thread.id, thread.sessionID != nil || thread.title != ThreadModel.untitled,
                  thread.updatedAt < cutoff
            else { continue }
            thread.settledAt = now
            thread.isPinned = false
            thread.endSession()
            changed = true
        }
        if changed { save() }
    }

    /// Settles a thread once every PR it's linked to has merged and it's had nothing new
    /// since: the work landed. Same exceptions as idle settling; one closed without merging,
    /// or still open, keeps it.
    func settleMergedThreads(now: Date = .now) {
        var changed = false
        for thread in threads {
            guard !thread.isSettled, !thread.isEphemeral, !thread.isSideChat, thread.autoSettle, thread.reactivatedAt == nil,
                  !thread.isWorking, thread.pendingPermissions.isEmpty, visibleThreadID != thread.id
            else { continue }
            let pulls = workedPullRequests(for: thread)
            guard !pulls.isEmpty else { continue }
            let merges = pulls.compactMap { $0.pull?.state == .merged ? ($0.pull?.mergedAt ?? now) : nil }
            // Work after the last merge (beyond the turn that merged it) is new work.
            guard merges.count == pulls.count, let last = merges.max(), thread.updatedAt <= last.addingTimeInterval(15 * 60) else { continue }
            thread.settledAt = now
            thread.isPinned = false
            thread.endSession()
            thread.closeBrowser()
            changed = true
        }
        if changed { save() }
    }

    /// Looks up the PRs of threads that might settle when they merge, including ones you
    /// aren't looking at.
    func syncPullRequestsForSettling() async {
        for thread in threads where !thread.isSettled && !thread.isEphemeral && !thread.isSideChat && thread.autoSettle
            && !thread.pullRequestLinks.isEmpty {
            let pulls = workedPullRequests(for: thread)
            guard pulls.contains(where: { $0.pull?.state != .merged }) else { continue }
            await syncPullRequests(for: thread)
        }
        settleMergedThreads()
    }

    /// Everything the sidebar shows, in order. Lets the list animate inserts and reorders.
    var sidebarOrder: [UUID] {
        projects.flatMap { project in
            guard !isCollapsed(project) else { return [project.id] }
            let settled = expandedShelves.contains(project.id) ? settledThreads(in: project).map(\.id) : []
            return [project.id] + threads(in: project).map(\.id) + settled
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
        var project = Project(name: url.lastPathComponent, path: url.path,
                              permissionMode: defaults.permissionMode ?? .standard)
        // A folder you had before gets its old ID back, and with it its memory.
        if let id = removedProjectIDs.removeValue(forKey: url.path), self.project(id: id) == nil { project.id = id }
        projects.append(project)
        newThread(in: project)
        refreshWorkspaces()
    }

    func removeProject(_ project: Project) {
        guard !isHome(project) else { return }
        for thread in threads(in: project) { deleteThread(thread, purge: true) }
        projects.removeAll { $0.id == project.id }
        workspaceModels[project.id] = nil
        if overviewProjectID == project.id { overviewProjectID = nil }
        for role in MemoryRole.allCases { closeMemoryAgent(role, projectID: project.id) }
        removedProjectIDs[project.path] = project.id
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

    /// Deletes a thread. Its files stay until the app quits, so Undo can bring it back;
    /// `purge` removes them right away.
    func deleteThread(_ thread: ThreadModel, purge: Bool = false) {
        guard let index = threads.firstIndex(where: { $0.id == thread.id }) else { return }
        thread.endSession()
        thread.closeBrowser()
        let wasSelected = selectedThreadID == thread.id
        if wasSelected { selectNeighbour(of: thread) }
        threads.remove(at: index)
        save()
        if purge {
            purgeFiles(of: thread.id)
            return
        }
        recentlyDeleted[thread.id] = thread
        offerUndo("Delete Thread", message: "Deleted “\(thread.title)”") { [weak self] in
            guard let self, self.recentlyDeleted.removeValue(forKey: thread.id) != nil else { return }
            self.threads.insert(thread, at: min(index, self.threads.count))
            if wasSelected { self.selectedThreadID = thread.id }
            self.save()
        }
    }

    private func purgeFiles(of id: UUID) {
        store.deleteTranscript(id)
        store.deleteAttachments(thread: id)
    }

    // MARK: - Undo

    /// What the last thread action did, with a way back, shown for a few seconds.
    struct UndoNotice: Identifiable {
        let id = UUID()
        let message: String
        let undo: @MainActor () -> Void
    }

    private(set) var undoNotice: UndoNotice?
    /// The window's, for ⌘Z and Edit → Undo.
    @ObservationIgnored weak var undoManager: UndoManager?
    /// Deleted threads, kept until quitting so Undo can bring them back.
    @ObservationIgnored private var recentlyDeleted: [UUID: ThreadModel] = [:]
    @ObservationIgnored private var undoNoticeExpiry: Task<Void, Never>?

    /// Shows `message` with Undo for five seconds, and puts the action on the window's undo
    /// stack. Undoing twice (the button, then ⌘Z) does nothing the second time.
    private func offerUndo(_ actionName: String, message: String, undo: @escaping @MainActor () -> Void) {
        var done = false
        let once: @MainActor () -> Void = { [weak self] in
            guard !done else { return }
            done = true
            withAnimation(.snappy) { undo() }
            if self?.undoNotice?.message == message { self?.dismissUndoNotice() }
        }
        undoManager?.registerUndo(withTarget: self) { _ in MainActor.assumeIsolated { once() } }
        undoManager?.setActionName(actionName)
        let notice = UndoNotice(message: message, undo: once)
        withAnimation(.snappy) { undoNotice = notice }
        undoNoticeExpiry?.cancel()
        undoNoticeExpiry = Task { [weak self] in
            try? await Task.sleep(for: .seconds(5))
            guard !Task.isCancelled, self?.undoNotice?.id == notice.id else { return }
            self?.dismissUndoNotice()
        }
    }

    func dismissUndoNotice() {
        withAnimation(.snappy) { undoNotice = nil }
    }

    /// Pin or unpin from the sidebar; unpinning offers Undo.
    func togglePinned(_ thread: ThreadModel) {
        let pinned = thread.isPinned
        setPinned(!pinned, thread)
        let restore: @MainActor () -> Void = { [weak self] in self?.setPinned(pinned, thread) }
        if pinned {
            offerUndo("Unpin Thread", message: "Unpinned “\(thread.title)”", undo: restore)
        } else {
            undoManager?.registerUndo(withTarget: self) { _ in MainActor.assumeIsolated { restore() } }
            undoManager?.setActionName("Pin Thread")
        }
    }

    /// Settle or un-settle from the sidebar. Undo puts it back as it was, pin included.
    func toggleSettled(_ thread: ThreadModel) {
        let settledAt = thread.settledAt, reactivatedAt = thread.reactivatedAt, pinned = thread.isPinned
        let wasSelected = selectedThreadID == thread.id
        setSettled(!thread.isSettled, thread)
        let restore: @MainActor () -> Void = { [weak self] in
            guard let self else { return }
            thread.settledAt = settledAt
            thread.reactivatedAt = reactivatedAt
            thread.isPinned = pinned
            if wasSelected { self.selectedThreadID = thread.id }
            self.save()
        }
        if settledAt == nil {
            offerUndo("Settle Thread", message: "Settled “\(thread.title)”", undo: restore)
        } else {
            undoManager?.registerUndo(withTarget: self) { _ in MainActor.assumeIsolated { restore() } }
            undoManager?.setActionName("Un-settle Thread")
        }
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
            gitlab: ready("glab").map { GitLabClient(executable: $0, environment: environment) },
            stacks: ready("gh").map { GitHubStacks(executable: $0, environment: environment) }
        )
    }

    func refreshWorkspaces(rescan: Bool = false, pullsMaxAge: TimeInterval = 120) {
        guard environmentReady else { return }
        let tools = workspaceTools
        for project in projects {
            guard let workspace = workspace(for: project) else { continue }
            Task {
                await workspace.refresh(tools: tools, rescan: rescan, pullsMaxAge: pullsMaxAge)
                noteRepoUsage(in: workspace)
            }
        }
    }

    // MARK: - A thread's pull requests

    /// A PR a thread is linked to: from a branch it works on, or a link that came up in it.
    struct ThreadPullRequest: Identifiable {
        let id: String
        let number: Int
        let repo: String
        let url: URL
        /// Nil until a linked PR has been looked up.
        let pull: PullRequest?
        /// The branch that ties it to the thread, when that's how it was found.
        let branch: String?
        /// The thread changed it (opened, merged, commented…) or its branch; otherwise it
        /// only came up.
        let isWorkedOn: Bool
    }

    /// What a stack's button is doing ("Merging…"), by stack ID.
    var stackActivity: [String: String] = [:]
    /// What went wrong with a stack's last action, by stack ID, until dismissed.
    var stackErrors: [String: String] = [:]
    /// Whether `gh stack` is installed; nil until asked.
    var stacksInstalled: Bool?

    /// The thread's PRs as their forge last described them, any state, by link ID.
    private var linkedPullRequests: [String: PullRequest] = [:]
    /// When each thread's PRs were last fetched.
    @ObservationIgnored private var pullsSyncedAt: [UUID: Date] = [:]
    @ObservationIgnored private var pullsSyncing: Set<UUID> = []
    private(set) var pullsSyncedAtObserved: [UUID: Date] = [:]

    /// Open PRs from the branches it works on, and every PR linked in it, whatever their
    /// state now: one merged since keeps showing, as merged.
    func pullRequests(for thread: ThreadModel) -> [ThreadPullRequest] {
        var result: [ThreadPullRequest] = [], seen = Set<String>()
        let openPulls = workspaceModels.values.flatMap(\.pullRequests)
        let worked = Set(thread.workedPullRequestLinks.compactMap { PullRequestLink.find(in: $0).first?.id })
        for pull in branchPullRequests(for: thread) where seen.insert(pull.pull.id).inserted {
            result.append(ThreadPullRequest(id: pull.pull.id, number: pull.pull.number, repo: pull.pull.remote.name, url: pull.pull.url,
                                            pull: linkedPullRequests[pull.pull.id] ?? pull.pull, branch: pull.branch, isWorkedOn: true))
        }
        for url in thread.pullRequestLinks.reversed() {
            guard let link = PullRequestLink.find(in: url).first, seen.insert(link.id).inserted else { continue }
            let pull = linkedPullRequests[link.id] ?? openPulls.first(where: link.matches)
            result.append(ThreadPullRequest(id: link.id, number: link.number, repo: link.remote.name, url: link.url, pull: pull, branch: nil,
                                            isWorkedOn: worked.contains(link.id)))
        }
        return result
    }

    /// Only the PRs the thread worked on: what its toolbar counts and what settles it.
    func workedPullRequests(for thread: ThreadModel) -> [ThreadPullRequest] {
        pullRequests(for: thread).filter(\.isWorkedOn)
    }

    /// The PR links a tool call's PR activity means: a URL as is; a number in the repo it
    /// names, or else the one the thread runs in; no PR named, the branch's own.
    func links(for activity: PullRequestActivity, in thread: ThreadModel) -> [String] {
        let workspace = project(id: thread.projectID).flatMap(workspace(for:))
        let here = workspace?.checkout(containing: thread.effectiveDirectory)
        return activity.targets.compactMap { target -> String? in
            switch target {
            case .link(let link):
                return link.url.absoluteString
            case .number(let number, let repo):
                let remote: GitRemote?
                if let repo {
                    remote = workspaceModels.values.lazy.flatMap(\.remotes.values).first { $0.path.lowercased() == repo.lowercased() }
                        ?? GitRemote(host: "github.com", path: repo, forge: .github)
                } else {
                    remote = here.flatMap { workspace?.remotes[$0.repo.id] }
                }
                guard let remote, let web = remote.webURL else { return nil }
                return web.absoluteString + (remote.forge == .gitlab ? "/-/merge_requests/\(number)" : "/pull/\(number)")
            case .currentBranch:
                guard let workspace, let here, let branch = workspace.branch(of: here) else { return nil }
                return workspace.pullRequests(for: here.repo).first { $0.headBranch == branch }?.url.absoluteString
            }
        }
    }

    /// Open PRs whose head branch is one the thread changed something on (not only read).
    private func branchPullRequests(for thread: ThreadModel) -> [(pull: PullRequest, branch: String)] {
        guard let project = project(id: thread.projectID), let workspace = workspace(for: project) else { return [] }
        var found: [(PullRequest, String)] = []
        for checkout in workspace.writtenCheckouts(for: thread) {
            let branch = checkout.worktree?.branch ?? (checkout.worktree == nil ? workspace.statuses[checkout.repo.id]?.branch : nil)
            guard let branch, branch != workspace.defaultBranches[checkout.repo.id] else { continue }
            found += workspace.pullRequests(for: checkout.repo).filter { $0.headBranch == branch }.map { ($0, branch) }
        }
        return found
    }

    /// Brings a thread's PRs up to date: PRs found through its branches are saved with it
    /// (so they stay after merging, when they leave the open list), then all of them are
    /// fetched, whatever their state. At most every few seconds unless forced.
    func syncPullRequests(for thread: ThreadModel, force: Bool = false) async {
        thread.addWorkedPullRequestLinks(branchPullRequests(for: thread).map(\.pull.url.absoluteString))
        let links = thread.pullRequestLinks.compactMap { PullRequestLink.find(in: $0).first }
        guard !links.isEmpty, !pullsSyncing.contains(thread.id) else { return }
        if !force, let last = pullsSyncedAt[thread.id], Date.now.timeIntervalSince(last) < 10 { return }
        pullsSyncing.insert(thread.id)
        defer { pullsSyncing.remove(thread.id) }
        let found: [PullRequest]
        if let mock = MockPullRequests.all {
            found = mock.filter { pull in links.contains { $0.matches(pull) } }
        } else if let github = workspaceTools.github {
            found = await github.fetch(links: links)
        } else {
            found = []
        }
        for pull in found where linkedPullRequests[pull.id] != pull { linkedPullRequests[pull.id] = pull }
        pullsSyncedAt[thread.id] = .now
        pullsSyncedAtObserved[thread.id] = .now
        // A PR that just changed state changes what the repos view lists too.
        if found.contains(where: { $0.state != .open }) { refreshWorkspaces(pullsMaxAge: 5) }
        settleMergedThreads()
    }

    /// Claude just did something to PRs (`gh pr merge`, a push): look again a moment later,
    /// and once more after, for checks and merges that take a little while.
    func pullRequestActivity(in thread: ThreadModel) {
        Task {
            for delay in [3.0, 12.0] {
                try? await Task.sleep(for: .seconds(delay))
                await syncPullRequests(for: thread, force: true)
            }
        }
    }

    // MARK: - Worktrees

    func worktreesExpanded(_ repo: WorkspaceRepo) -> Bool {
        expandedWorktrees.contains(WorkspaceModel.realPath(repo.url.path))
    }

    func setWorktreesExpanded(_ expanded: Bool, for repos: [WorkspaceRepo]) {
        for repo in repos {
            let path = WorkspaceModel.realPath(repo.url.path)
            if expanded { expandedWorktrees.insert(path) } else { expandedWorktrees.remove(path) }
        }
        save()
    }

    /// How many threads have worked in a repo, its worktrees included.
    func usage(of repo: WorkspaceRepo) -> Int {
        repoUsage[WorkspaceModel.realPath(repo.url.path)]?.count ?? 0
    }

    /// Records which threads work in which repos of `workspace`: every thread, wherever it
    /// lives, since a Home thread can work in a project's repo too.
    func noteRepoUsage(in workspace: WorkspaceModel) {
        guard workspace.statusUpdatedAt != nil else { return }
        record(workspace.checkoutsByThread(threads))
    }

    /// A thread touched new folders: credit the repos they're in.
    func footprintChanged(_ thread: ThreadModel) {
        guard !thread.isSideChat else { return }
        // Only once a workspace knows its worktrees: until then a worktree nested in the
        // project looks like a repo of its own, and would be counted as one.
        for workspace in workspaceModels.values where workspace.statusUpdatedAt != nil {
            record([thread.id: workspace.checkouts(for: thread).map { WorkspaceModel.realPath($0.repo.url.path) }], dates: [thread.id: thread.updatedAt])
        }
    }

    private func record(_ reposByThread: [UUID: [String]], dates: [UUID: Date]? = nil) {
        var usage = repoUsage
        for (thread, repos) in reposByThread {
            let date = dates?[thread] ?? threads.first { $0.id == thread }?.updatedAt
            for repo in Set(repos) {
                var entry = usage[repo] ?? RepoUsage()
                entry.threads.insert(thread)
                if let date, date > (entry.lastUsed ?? .distantPast) { entry.lastUsed = date }
                usage[repo] = entry
            }
        }
        guard usage != repoUsage else { return }
        repoUsage = usage
        save()
    }

    /// A thread that runs in `folder` (a worktree) rather than the project folder.
    func newThread(in project: Project, workingIn folder: String) {
        let thread = newThread(in: project)
        thread.setWorkingDirectory(folder == project.path ? nil : folder)
    }

    /// Where New Worktree puts a checkout unless told otherwise: the app's own storage,
    /// outside the project, so its agents don't find every file twice.
    func defaultWorktreeLocation(repo: WorkspaceRepo, branch: String) -> URL {
        let slug = branch.replacingOccurrences(of: "/", with: "-").replacingOccurrences(of: " ", with: "-")
        return store.root.appending(path: "worktrees").appending(path: repo.url.lastPathComponent).appending(path: slug)
    }

    /// `git worktree add`; nil on success, else git's complaint.
    func addWorktree(to repo: WorkspaceRepo, in workspace: WorkspaceModel, branch: String, from base: String?, at location: URL) async -> String? {
        guard let git = workspaceTools.git else { return "Git isn't available." }
        try? FileManager.default.createDirectory(at: location.deletingLastPathComponent(), withIntermediateDirectories: true)
        let exists = await git.branchExists(branch, in: repo.url)
        let error = await git.addWorktree(of: repo.url, at: location, newBranch: exists ? nil : branch,
                                          from: exists ? nil : base, existingBranch: exists ? branch : nil)
        await workspace.refreshStatus(tools: workspaceTools, minInterval: 0)
        return error
    }

    /// `git worktree remove`, and nothing else: local, quick even offline. Nil on success,
    /// else git's complaint.
    func removeWorktree(_ worktree: Worktree, of repo: WorkspaceRepo, in workspace: WorkspaceModel, force: Bool) async -> String? {
        guard let git = workspaceTools.git else { return "Git isn't available." }
        if let error = await git.removeWorktree(worktree, of: repo.url, force: force) { return error }
        workspace.forgetWorktree(worktree, in: repo)
        return nil
    }

    /// `git worktree prune`: forgets worktrees whose folders are gone. Nil on success.
    func pruneWorktrees(of repo: WorkspaceRepo, in workspace: WorkspaceModel) async -> String? {
        guard let git = workspaceTools.git else { return "Git isn't available." }
        if let error = await git.pruneWorktrees(of: repo.url) { return error }
        workspace.forgetMissingWorktrees(in: repo)
        return nil
    }

    /// Threads saved before footprints were kept work theirs out, so the repos view knows
    /// which worktrees they're in.
    func computeFootprints() {
        for thread in threads { thread.computeFootprintIfNeeded() }
    }

    func openInTerminal(_ folder: URL) {
        let terminal = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.apple.Terminal")
            ?? URL(fileURLWithPath: "/System/Applications/Utilities/Terminal.app")
        NSWorkspace.shared.open([folder], withApplicationAt: terminal, configuration: NSWorkspace.OpenConfiguration())
    }

    /// An agent may have edited files, committed, or opened a PR.
    func turnFinished(in projectID: UUID) {
        guard let project = project(id: projectID), let workspace = workspace(for: project) else { return }
        let tools = workspaceTools
        Task {
            await workspace.refreshStatus(tools: tools, minInterval: 0)
            noteRepoUsage(in: workspace)
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
                guard let self else { continue }
                self.settleIdleThreads()
                await self.syncPullRequestsForSettling()
                self.closeIdleBrowsers()
                guard NSApp.isActive else { continue }
                self.refreshWorkspaces(pullsMaxAge: 240)
            }
        }
    }

    // MARK: - Launching

    func launchOptions(for thread: ThreadModel) async throws -> ClaudeLaunchOptions {
        await bootstrap?.value
        guard let executable = claudeExecutable else { throw HarnessError.claudeNotFound }
        guard let project = project(id: thread.projectID) else { throw HarnessError.projectMissing }
        var options = ClaudeLaunchOptions(
            executable: executable,
            // A worktree that's since been removed falls back to the project.
            workingDirectory: thread.workingDirectory.flatMap { FileManager.default.fileExists(atPath: $0) ? URL(fileURLWithPath: $0, isDirectory: true) : nil } ?? project.url,
            environment: environment,
            model: thread.effectiveModel,
            permissionMode: project.permissionMode.cliValue,
            effort: thread.effectiveEffort,
            resumeSessionID: thread.sessionID,
            appendSystemPrompt: diagramInstructions ? AgentInstructions.diagrams : nil
        )
        if memoryEnabled && claudeCodeMemoryOff {
            // One memory: the app's. Claude Code's own files would compete with it.
            options.environment["CLAUDE_CODE_DISABLE_AUTO_MEMORY"] = "1"
        }
        if let role = thread.memoryRole {
            return memorySpecialistOptions(options, role: role, thread: thread, project: project)
        }
        // Claude Code keeps its task tools from current models unless asked; they're what
        // the plan checklist shows, and cost nothing until used (they load on demand).
        if planChecklist, !thread.isSideChat { options.environment["CLAUDE_CODE_ENABLE_TODO_TOOLS"] = "1" }
        if !thread.isSideChat {
            // Showing you a file changes nothing: it doesn't ask.
            options.allowedTools.append("mcp__harness")
            options.appendSystemPrompt = [options.appendSystemPrompt, AgentInstructions.media].compactMap(\.self).joined(separator: "\n\n")
        }
        await checkStacksInstalled()
        if stacksInstalled == true, !thread.isSideChat {
            options.appendSystemPrompt = [options.appendSystemPrompt, AgentInstructions.stacks].compactMap(\.self).joined(separator: "\n\n")
        }
        if browserTools {
            // The browser is the app's own and touches none of your files: it doesn't ask.
            options.allowedTools.append("mcp__browser")
            let instructions = thread.isSideChat ? AgentInstructions.browserReadOnly : AgentInstructions.browser
            options.appendSystemPrompt = [options.appendSystemPrompt, instructions].compactMap(\.self).joined(separator: "\n\n")
        }
        if memoryEnabled {
            // Memory tools only touch the app's own database: they don't ask.
            options.allowedTools.append("mcp__memory")
            if thread.isSideChat {
                options.appendSystemPrompt = [options.appendSystemPrompt, AgentInstructions.memoryReadOnly].compactMap(\.self).joined(separator: "\n\n")
            } else {
                let prompt = await memoryPrompt(for: project, thread: thread)
                options.appendSystemPrompt = [options.appendSystemPrompt, prompt.text].compactMap(\.self).joined(separator: "\n\n")
            }
        }
        if thread.isSideChat {
            // A copy of the thread's conversation that can look but not edit, and leaves
            // nothing behind: no edit tools, no MCP servers, nothing saved. It reads freely;
            // commands and web requests ask you first, whatever the project's mode.
            if thread.sessionID == nil, let source = thread.forksSession {
                options.resumeSessionID = source
                options.forkSession = true
            }
            // Spelled out: left unset, a defaultMode in your Claude settings would apply.
            options.permissionMode = "default"
            options.tools = ["Read", "Glob", "Grep", "Bash", "WebFetch", "WebSearch"]
            options.withoutMCP = true
            options.persistsSession = false
            options.appendSystemPrompt = [options.appendSystemPrompt, AgentInstructions.sideChat].compactMap(\.self).joined(separator: "\n\n")
        }
        return options
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

    func updateCatalog(_ catalog: ClaudeCatalog, projectID: UUID? = nil) {
        var changed = false
        if !catalog.models.isEmpty, catalog.models != claudeModels {
            claudeModels = catalog.models
            changed = true
        }
        if let projectID, !catalog.commands.isEmpty, slashCommands[projectID] != catalog.commands {
            slashCommands[projectID] = catalog.commands
            changed = true
        }
        if changed { save() }
    }

    // MARK: - Slash commands

    /// By project: commands and skills depend on the folder (its `.claude/commands`, skills).
    private(set) var slashCommands: [UUID: [SlashCommand]] = [:]
    @ObservationIgnored private var commandLoads: Set<UUID> = []

    /// Commands the app handles itself.
    static let appCommands = [
        SlashCommand(name: "btw", description: "Ask a side question in the sidebar. Claude sees this thread; nothing is added to it.",
                     argumentHint: "<question>", isBuiltin: true),
    ]

    /// By project: the skills a session there reported, which the agent can run itself.
    private(set) var skillNames: [UUID: [String]] = [:]

    func noteSkills(_ skills: [String], projectID: UUID) {
        guard skillNames[projectID] != skills else { return }
        skillNames[projectID] = skills
        save()
    }

    /// Commands the agent can run itself partway through a message: skills, by what a
    /// session reported; until one has, your own commands and skills.
    func chainableCommands(for projectID: UUID) -> [SlashCommand] {
        let commands = slashCommands(for: projectID)
        guard let skills = skillNames[projectID].map(Set.init) else { return commands.filter { !$0.isBuiltin } }
        return commands.filter { command in skills.contains(command.name) || command.aliases.contains(where: skills.contains) }
    }

    /// Your message as the agent gets it: with skills named later in it (`then /review`),
    /// a note to run them as they come up. Claude Code only runs the one a message starts with.
    func messageForAgent(_ text: String, projectID: UUID) -> String {
        let known = Set(chainableCommands(for: projectID).flatMap { [$0.name] + $0.aliases })
        guard let note = SlashCommand.chainNote(for: SlashCommand.chainedCommands(in: text, known: known)) else { return text }
        return text + "\n\n" + note
    }

    /// What the "/" menu offers in a project: the app's own, then the CLI's.
    func slashCommands(for projectID: UUID) -> [SlashCommand] {
        let own = Set(Self.appCommands.map(\.name))
        return Self.appCommands + (slashCommands[projectID] ?? []).filter {
            !own.contains($0.name) && !SlashCommand.terminalOnly.contains($0.name)
        }
    }

    /// Lists a project's commands with a throwaway CLI process (no request is made), once
    /// per launch, so the menu is right even before a thread there has started a session.
    func loadSlashCommands(for projectID: UUID) {
        guard !commandLoads.contains(projectID), let project = project(id: projectID) else { return }
        commandLoads.insert(projectID)
        Task {
            await bootstrap?.value
            guard let executable = claudeExecutable,
                  let catalog = await ClaudeCatalog.load(executable: executable, environment: environment, workingDirectory: project.url)
            else { return }
            updateCatalog(catalog, projectID: projectID)
        }
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
    func setPlanChecklist(_ enabled: Bool) {
        planChecklist = enabled
        save()
        for thread in threads { thread.restartSession() }
    }

    /// The tools and their instructions are part of the launch: running sessions restart.
    func setBrowserTools(_ enabled: Bool) {
        browserTools = enabled
        save()
        for thread in threads { thread.restartSession() }
    }

    func setBrowserAutoShow(_ enabled: Bool) {
        browserAutoShow = enabled
        save()
    }

    /// Each idle page is a web content process: tens of MB for nothing. Closed, its tabs'
    /// addresses wait to reopen when the browser is next used.
    func closeIdleBrowsers() {
        for thread in threads where thread.browser?.isIdle == true && !thread.isWorking && visibleThreadID != thread.id {
            thread.closeBrowser()
        }
    }

    /// The agent opened a page: show it, if that's the thread you're looking at.
    func revealBrowser(for thread: ThreadModel?) {
        guard browserAutoShow, let thread, !thread.isSideChat, visibleThreadID == thread.id else { return }
        showInspector(.browser)
    }

    func saveBrowserScreenshot(_ data: Data, thread: UUID?, toolUseID: String) {
        guard let thread else { return }
        store.saveBrowserScreenshot(data, thread: thread, toolUseID: toolUseID)
    }

    /// The tools a thread's session serves from inside the app.
    func mcpServers(for thread: ThreadModel) -> [any MCPToolServer] {
        if let role = thread.memoryRole {
            return [memoryServer(for: thread, access: .specialist, actor: .init(role == .importer ? "importer" : "reconciler", threadID: thread.id.uuidString))]
        }
        if thread.isSideChat {
            var servers: [any MCPToolServer] = []
            if browserTools, let parent = thread.parentThread {
                servers.append(BrowserToolServer(browser: BrowserTools(thread: parent, app: self, readOnly: true)))
            }
            if memoryEnabled { servers.append(memoryServer(for: thread, access: .readOnly, actor: .init("agent", threadID: thread.parentThread?.id.uuidString))) }
            return servers
        }
        var servers: [any MCPToolServer] = [HarnessToolServer(media: MediaTool(thread: thread, app: self))]
        if browserTools { servers.append(BrowserToolServer(browser: BrowserTools(thread: thread, app: self, readOnly: false))) }
        if memoryEnabled { servers.append(memoryServer(for: thread, access: .agent, actor: .init("agent", threadID: thread.id.uuidString))) }
        return servers
    }

    func setNotifications(_ enabled: Bool) {
        notifications = enabled
        save()
    }

    /// Tells you about a thread you aren't looking at. A finished turn only while the app
    /// is in the background (the sidebar marks it otherwise); Claude waiting for you,
    /// whenever it's another thread.
    func notify(_ kind: Notifier.Kind, about thread: ThreadModel, _ body: String) {
        guard notifications, !thread.isAside else { return }
        let isLooking = NSApp.isActive && visibleThreadID == thread.id
        guard !isLooking, kind == .needsYou || !NSApp.isActive else { return }
        notifier.post(kind, thread: thread.id, title: thread.title, subtitle: project(id: thread.projectID)?.name ?? "",
                      body: body)
    }

    private func openFromNotification(_ id: UUID) {
        guard let thread = threads.first(where: { $0.id == id }) else { return }
        if thread.isSettled { setSettled(false, thread) }
        selectedThreadID = id
        NSApp.activate()
        let window = NSApp.windows.first { $0.identifier?.rawValue == "main" } ?? NSApp.windows.first { $0.canBecomeMain }
        if window?.isMiniaturized == true { window?.deminiaturize(nil) }
        window?.makeKeyAndOrderFront(nil)
    }

    /// Back in the app: the thread on screen has been seen.
    func didBecomeActive() {
        if let visibleThreadID { notifier.clear(thread: visibleThreadID) }
    }

    /// The Dock icon counts threads waiting for your approval, answer or plan review.
    func waitingChanged() {
        let waiting = threads.count { !$0.pendingPermissions.isEmpty || $0.sideChat?.pendingPermissions.isEmpty == false }
        notifier.setBadge(waiting)
    }

    func setDiagramInstructions(_ enabled: Bool) {
        diagramInstructions = enabled
        save()
        for thread in threads { thread.restartSession() }
    }

    // MARK: - Titles

    enum TitleSource {
        /// A new thread's first message; retried twice, as it's the one you'll see.
        case firstMessage(String)
        /// The whole conversation, replacing the current title only if it's improved.
        case conversation
    }

    func setTitleGeneration(_ settings: TitleGenerationSettings) {
        titleGeneration = settings
        titleGenerationError = nil
        save()
    }

    func canGenerateTitles(with provider: TitleGenerationSettings.Provider) -> Bool {
        cliStatuses[provider.cli.id]?.isReady == true
    }

    /// Names a thread in the background with the provider from Settings, as T3 Code does:
    /// a one-shot request with no tools, in an empty folder.
    func generateTitle(for thread: ThreadModel, from source: TitleSource) {
        guard !thread.isEphemeral, let provider = titleGeneration.provider, let model = titleGeneration.effectiveModel else { return }
        let regenerating: Bool
        let messages: [ThreadTitlePrompt.Message]
        switch source {
        case .firstMessage(let text):
            regenerating = false
            messages = [ThreadTitlePrompt.Message(role: .user, text: text)]
        case .conversation:
            regenerating = true
            messages = thread.titleMessages
            guard messages.contains(where: { $0.role == .user }) else { return }
        }
        let previousTitle = thread.title
        let request = thread.beginTitleGeneration()
        Task {
            await bootstrap?.value
            guard let executable = cliStatuses[provider.cli.id].flatMap({ $0.isReady ? $0.path : nil }) else {
                titleGenerationError = "\(provider.cli.name) isn't available."
                thread.finishTitleGeneration(request, with: nil, regenerating: regenerating)
                return
            }
            let environment = environment
            let text = regenerating ? ThreadTitlePrompt.context(for: messages) : messages[0].text
            let linked = await TitleLinkedContext.resolve(message: text, gh: toolStatuses["gh"].flatMap { $0.isReady ? $0.path : nil },
                environment: environment)
            let prompt = regenerating
                ? ThreadTitlePrompt.regenerate(contents: text, previousTitle: previousTitle, linkedContext: linked)
                : ThreadTitlePrompt.initial(message: text, linkedContext: linked)

            var result = await TitleGenerator.generate(
                prompt: prompt, provider: provider, model: model, executable: executable, environment: environment)
            for attempt in regenerating ? 0..<0 : 0..<2 {
                guard case .failure = result else { break }
                try? await Task.sleep(for: .seconds(2 << attempt))
                result = await TitleGenerator.generate(
                    prompt: prompt, provider: provider, model: model, executable: executable, environment: environment)
            }
            switch result {
            case .success(let title):
                titleGenerationError = nil
                thread.finishTitleGeneration(request, with: title, regenerating: regenerating)
            case .failure(let error):
                titleGenerationError = error.description
                thread.finishTitleGeneration(request, with: nil, regenerating: regenerating)
            }
        }
    }

    // MARK: - Resuming Claude Code sessions

    enum SessionImportError: LocalizedError, Equatable {
        case notFound
        case unreadable
        case folderMissing(String)

        var errorDescription: String? {
            switch self {
            case .notFound: "No Claude Code session with this ID."
            case .unreadable: "Couldn't read this session's history."
            case .folderMissing(let path): "The session's folder no longer exists: \(path)"
            }
        }
    }

    /// Shows the "Resume Claude Code Session" sheet.
    var isResumingSession = false

    /// The project's files for `@` mentions: listed in the background, kept a little while,
    /// so typing a mention never waits on the disk. In a git repo, what git sees (nothing
    /// ignored), plus the files of repos cloned inside it; otherwise a walk of the folder.
    func fileIndex(for projectID: UUID) async -> FileIndex? {
        if let cached = fileIndexes[projectID], cached.listedAt.timeIntervalSinceNow > -30 { return cached.index }
        // Your whole home folder isn't a project to pick files from.
        guard let project = project(id: projectID), project.url.standardizedFileURL.path != NSHomeDirectory() else { return nil }
        if let listing = fileListings[projectID] { return await listing.value }
        await bootstrap?.value
        let git = workspaceTools.git
        let root = project.url
        let nested = workspace(for: project)?.repos.filter { !$0.isRoot && $0.isCloned } ?? []
        let listing = Task.detached(priority: .userInitiated) { () -> FileIndex in
            guard let git, var files = await git.files(in: root) else { return FileIndex(files: FileIndex.walk(root)) }
            for repo in nested {
                files += (await git.files(in: repo.url) ?? []).map { repo.relativePath + "/" + $0 }
            }
            return FileIndex(files: files)
        }
        fileListings[projectID] = listing
        let index = await listing.value
        fileListings[projectID] = nil
        fileIndexes[projectID] = (index, .now)
        return index
    }

    /// The thread that already continues this session, if any.
    func thread(forSession sessionID: String) -> ThreadModel? {
        threads.first { $0.sessionID?.lowercased() == sessionID.lowercased() }
    }

    /// Finds and reads a session from Claude Code's history, off the main thread.
    func loadClaudeSession(_ sessionID: String) async -> Result<ClaudeSessionArchive, SessionImportError> {
        await bootstrap?.value
        let config = ClaudeSessionArchive.configDirectory(environment: environment)
        return await Task.detached(priority: .userInitiated) {
            guard let url = ClaudeSessionArchive.locate(sessionID: sessionID, configDirectory: config) else {
                return .failure(.notFound)
            }
            do {
                return .success(try ClaudeSessionArchive.load(from: url, sessionID: sessionID))
            } catch {
                return .failure(.unreadable)
            }
        }.value
    }

    /// What the session's history has beyond `known`, read off the main thread. With
    /// `changedSince`, nil unless the file changed after that.
    func sessionBackfill(sessionID: String, known: [ClaudeSessionArchive.Item], changedSince: Date?) async -> SessionBackfill? {
        await bootstrap?.value
        let config = ClaudeSessionArchive.configDirectory(environment: environment)
        return await Task.detached(priority: .utility) {
            guard let url = ClaudeSessionArchive.locate(sessionID: sessionID, configDirectory: config) else { return nil }
            if let changedSince {
                // A second's grace: the file's last line can land just after our save.
                let modified = (try? FileManager.default.attributesOfItem(atPath: url.path)[.modificationDate] as? Date) ?? .distantPast
                guard modified > changedSince.addingTimeInterval(1) else { return nil }
            }
            guard let archive = try? ClaudeSessionArchive.load(from: url, sessionID: sessionID) else { return nil }
            let backfill = SessionBackfill(known: known, archive: archive)
            return backfill.isEmpty ? nil : backfill
        }.value
    }

    /// Opens a thread that shows the session's history and continues it with `--resume`.
    /// It goes in `destination`, or the project for the folder it ran in (added if you don't
    /// have one yet). Either way it runs in that folder: `--resume` only finds it from there.
    @discardableResult
    func resumeClaudeSession(_ archive: ClaudeSessionArchive, into destination: Project? = nil) throws(SessionImportError) -> ThreadModel {
        if let existing = thread(forSession: archive.sessionID) {
            if existing.isSettled { setSettled(false, existing) }
            selectedThreadID = existing.id
            return existing
        }
        let folder = try sessionFolder(archive.cwd)
        let project: Project
        if let destination { project = destination } else { project = try self.project(forSessionFolder: archive.cwd) }
        if isCollapsed(project) { setCollapsed(false, for: project) }

        var meta = ThreadMeta(
            id: UUID(), projectID: project.id,
            title: archive.title ?? archive.firstMessage.map(ThreadTitlePrompt.seed) ?? ThreadModel.untitled,
            createdAt: archive.startedAt ?? .now, updatedAt: archive.updatedAt ?? .now,
            sessionID: archive.sessionID,
            // Fresh in the list and safe from auto-settling until you send something.
            reactivatedAt: .now
        )
        if let folder, folder != Self.standardized(project.path) { meta.workingDirectory = folder }
        let thread = ThreadModel(meta: meta, app: self)
        thread.importTranscript(archive.items.map(TranscriptEntry.init(item:)))
        threads.append(thread)
        selectedThreadID = thread.id
        save()
        // Claude Code titles a session from its first message and rarely revisits it; a
        // name you gave it with /rename is kept as is.
        if !archive.hasCustomTitle { generateTitle(for: thread, from: .conversation) }
        return thread
    }

    private func project(forSessionFolder cwd: String?) throws(SessionImportError) -> Project {
        guard let path = try sessionFolder(cwd) else { return homeProject }
        if let existing = projects.first(where: { Self.standardized($0.path) == path }) {
            return existing
        }
        let project = Project(name: URL(fileURLWithPath: path).lastPathComponent, path: path,
                              permissionMode: defaults.permissionMode ?? .standard)
        projects.append(project)
        refreshWorkspaces()
        return project
    }

    /// The folder a session ran in, if it said, as long as it's still there.
    private func sessionFolder(_ cwd: String?) throws(SessionImportError) -> String? {
        guard let cwd else { return nil }
        let path = Self.standardized(cwd)
        var isDirectory: ObjCBool = false
        guard FileManager.default.fileExists(atPath: path, isDirectory: &isDirectory), isDirectory.boolValue else {
            throw .folderMissing(cwd)
        }
        return path
    }

    /// Where a resumed session goes unless you pick: the project for its folder, else the
    /// innermost project its folder is inside (a workspace), else nil, a new project for the
    /// folder. Home holds every folder under it, so it only counts as itself.
    func suggestedProject(forSessionFolder cwd: String?) -> Project? {
        guard let cwd else { return homeProject }
        return Self.project(containing: cwd, in: projects, home: homeProjectID)
    }

    nonisolated static func project(containing folder: String, in projects: [Project], home: UUID) -> Project? {
        let path = standardized(folder)
        if let exact = projects.first(where: { standardized($0.path) == path }) { return exact }
        return projects
            .filter { $0.id != home && path.hasPrefix(standardized($0.path) + "/") }
            .max { standardized($0.path).count < standardized($1.path).count }
    }

    nonisolated private static func standardized(_ path: String) -> String {
        URL(fileURLWithPath: path).standardizedFileURL.path
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

    /// Saves a moment later, so a burst of changes (a selection, a rename, a draft) is one
    /// save. Quitting saves right away.
    func save() {
        guard !saveScheduled else { return }
        saveScheduled = true
        Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(300))
            self?.saveNow()
        }
    }

    @ObservationIgnored private var saveScheduled = false

    func saveNow() {
        saveScheduled = false
        var state = PersistedState()
        state.projects = projects
        state.threads = threads.filter { !$0.isEphemeral }.map(\.meta)
        state.selectedThreadID = selectedThreadID
        state.overviewProjectID = overviewProjectID
        state.homeProjectID = homeProjectID
        state.cliPathOverrides = cliPathOverrides
        state.defaults = defaults
        state.claudeModels = claudeModels
        state.slashCommands = Dictionary(uniqueKeysWithValues: slashCommands.map { ($0.uuidString, $1) })
        state.skills = Dictionary(uniqueKeysWithValues: skillNames.map { ($0.uuidString, $1) })
        state.contextWindows = contextWindows
        state.rateLimit = rateLimit
        state.rateLimitUpdatedAt = rateLimitUpdatedAt
        state.collapsedProjectIDs = collapsedProjectIDs
        state.diagramInstructions = diagramInstructions
        state.autoSettleDays = autoSettleDays
        state.titleGeneration = titleGeneration
        state.showsInspector = showsInspector
        state.appearance = appearance
        state.notifications = notifications
        state.planChecklist = planChecklist
        state.browserTools = browserTools
        state.repoUsage = repoUsage
        state.expandedWorktrees = expandedWorktrees
        state.browserAutoShow = browserAutoShow
        state.memoryProjectID = memoryProjectID
        state.memoryEnabled = memoryEnabled
        state.claudeCodeMemoryOff = claudeCodeMemoryOff
        state.removedProjectIDs = removedProjectIDs
        state.memoryAgentModel = memoryAgentModel
        state.memoryAgentEffort = memoryAgentEffort
        store.saveState(state)
    }

    func shutdown() {
        for id in recentlyDeleted.keys { purgeFiles(of: id) }
        for thread in threads { thread.prepareForQuit() }
        saveNow()
        for thread in threads { thread.endSession() }
        store.flush()
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
