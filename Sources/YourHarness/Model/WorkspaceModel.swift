import Foundation
import HarnessCore

/// The CLIs a workspace refresh drives, resolved once discovery finishes.
struct WorkspaceTools: Sendable {
    var git: Git?
    var github: GitHubClient?
    var gitlab: GitLabClient?
}

/// Read-only view of one project's repos and their open PRs. Everything here is
/// discovered; nothing is written to the project.
@MainActor @Observable
final class WorkspaceModel {
    let projectID: UUID
    let root: URL
    private(set) var layout: WorkspaceLayout?
    /// Keyed by `WorkspaceRepo.id`.
    private(set) var statuses: [String: RepoStatus] = [:]
    private(set) var remotes: [String: GitRemote] = [:]
    private(set) var pullRequests: [PullRequest] = []
    private(set) var forgeErrors: [Forge: String] = [:]
    /// Signed-in user per forge, as its CLI reported it.
    private(set) var forgeAccounts: [Forge: String] = [:]
    private(set) var unreachable: Set<GitRemote> = []
    private(set) var isScanning = false
    private(set) var isRefreshingStatus = false
    private(set) var isRefreshingPulls = false
    private(set) var statusUpdatedAt: Date?
    private(set) var pullsUpdatedAt: Date?

    init(projectID: UUID, root: URL) {
        self.projectID = projectID
        self.root = root
    }

    var isRefreshing: Bool { isScanning || isRefreshingStatus || isRefreshingPulls }
    var repos: [WorkspaceRepo] { layout?.repos ?? [] }
    var clonedRepos: [WorkspaceRepo] { repos.filter(\.isCloned) }
    var unclonedRepos: [WorkspaceRepo] { repos.filter { !$0.isCloned } }
    var hasForgeRemotes: Bool { remotes.values.contains { $0.forge != nil } }

    func pullRequests(for repo: WorkspaceRepo) -> [PullRequest] {
        guard let remote = remotes[repo.id] else { return [] }
        return pullRequests.filter { $0.remote == remote }
    }

    func repo(for pullRequest: PullRequest) -> WorkspaceRepo? {
        repos.first { remotes[$0.id] == pullRequest.remote }
    }

    /// PR stacks split into what needs you, what's yours, and everything else.
    var pullRequestSections: (needsReview: [PullRequestStack], mine: [PullRequestStack], others: [PullRequestStack]) {
        var needsReview: [PullRequestStack] = [], mine: [PullRequestStack] = [], others: [PullRequestStack] = []
        for stack in PullRequestStack.build(pullRequests) {
            let members = stack.all
            if members.contains(where: \.reviewRequestedFromMe) {
                needsReview.append(stack)
            } else if members.contains(where: \.isMine) {
                mine.append(stack)
            } else {
                others.append(stack)
            }
        }
        return (needsReview, mine, others)
    }

    // MARK: - Refreshing

    /// Rescans the folder when asked or on first use, then refreshes status and, when
    /// stale, PRs. Cheap to call often: each part throttles itself.
    func refresh(tools: WorkspaceTools, rescan: Bool = false, pullsMaxAge: TimeInterval = 120) async {
        if layout == nil || rescan { await scan(tools: tools) }
        async let status: Void = refreshStatus(tools: tools)
        async let pulls: Void = refreshPulls(tools: tools, maxAge: pullsMaxAge)
        _ = await (status, pulls)
    }

    private func scan(tools: WorkspaceTools) async {
        guard !isScanning else { return }
        isScanning = true
        defer { isScanning = false }
        let root = root
        let layout = await Task.detached(priority: .userInitiated) { WorkspaceScanner.scan(root) }.value
        self.layout = layout

        guard let git = tools.git else { return }
        let found = await Self.map(layout.clonedRepos) { repo in
            (repo.id, await git.originURL(of: repo.url).flatMap(GitRemote.init(url:)))
        }
        remotes = Dictionary(found.compactMap { id, remote in remote.map { (id, $0) } }, uniquingKeysWith: { $1 })
    }

    func refreshStatus(tools: WorkspaceTools, minInterval: TimeInterval = 3) async {
        guard let git = tools.git, !isRefreshingStatus else { return }
        if let updated = statusUpdatedAt, Date.now.timeIntervalSince(updated) < minInterval { return }
        isRefreshingStatus = true
        defer { isRefreshingStatus = false }
        let results = await Self.map(clonedRepos) { repo in (repo.id, await git.status(of: repo.url)) }
        var statuses: [String: RepoStatus] = [:]
        for (id, status) in results { statuses[id] = status }
        if statuses != self.statuses { self.statuses = statuses }
        statusUpdatedAt = .now
    }

    func refreshPulls(tools: WorkspaceTools, maxAge: TimeInterval) async {
        guard !isRefreshingPulls else { return }
        if let updated = pullsUpdatedAt, Date.now.timeIntervalSince(updated) < maxAge { return }
        let byForge = Dictionary(grouping: Set(remotes.values), by: \.forge)
        guard byForge.keys.contains(where: { $0 != nil }) else { return }
        isRefreshingPulls = true
        defer { isRefreshingPulls = false }

        async let github = Self.fetch(byForge[.github], with: tools.github, forge: .github)
        async let gitlab = Self.fetch(byForge[.gitlab], with: tools.gitlab, forge: .gitlab)
        let snapshots = await [github, gitlab].compactMap { $0 }

        var errors: [Forge: String] = [:]
        var accounts: [Forge: String] = [:]
        var pulls: [PullRequest] = []
        var unreachable: Set<GitRemote> = []
        for snapshot in snapshots {
            if let error = snapshot.error { errors[snapshot.forge] = error }
            if let account = snapshot.accounts.values.first { accounts[snapshot.forge] = account }
            pulls += snapshot.pullRequests
            unreachable.formUnion(snapshot.unreachable)
        }
        // Keep what we had when a forge fails outright (offline), rather than blanking the list.
        let failed = Set(errors.keys).subtracting(snapshots.filter { !$0.pullRequests.isEmpty }.map(\.forge))
        pulls += pullRequests.filter { $0.remote.forge.map(failed.contains) ?? false }
        pulls.sort { ($0.updatedAt ?? .distantPast) > ($1.updatedAt ?? .distantPast) }

        if pulls != pullRequests { pullRequests = pulls }
        forgeErrors = errors
        forgeAccounts = accounts
        self.unreachable = unreachable
        pullsUpdatedAt = .now
    }

    private static func fetch(_ remotes: [GitRemote]?, with client: (some ForgeFetching)?, forge: Forge) async -> ForgeSnapshot? {
        guard let remotes, !remotes.isEmpty else { return nil }
        guard let client else {
            var snapshot = ForgeSnapshot(forge: forge)
            snapshot.error = "Install `\(forge.cliName)` to see \(forge.requestNoun)s from \(forge.displayName)."
            return snapshot
        }
        return await client.fetch(remotes)
    }

    /// Runs `transform` over `items` with at most `limit` running at once.
    nonisolated static func map<T: Sendable, R: Sendable>(
        _ items: [T], limit: Int = 8, _ transform: @escaping @Sendable (T) async -> R
    ) async -> [R] {
        await withTaskGroup(of: R.self) { group in
            var iterator = items.makeIterator()
            for _ in 0..<limit {
                guard let item = iterator.next() else { break }
                group.addTask { await transform(item) }
            }
            var results: [R] = []
            while let result = await group.next() {
                results.append(result)
                if let item = iterator.next() { group.addTask { await transform(item) } }
            }
            return results
        }
    }
}

protocol ForgeFetching: Sendable {
    func fetch(_ remotes: [GitRemote]) async -> ForgeSnapshot
}

extension GitHubClient: ForgeFetching {}
extension GitLabClient: ForgeFetching {}
