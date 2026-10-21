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
    /// Each repo's linked worktrees (not its main checkout), keyed by `WorkspaceRepo.id`.
    private(set) var worktrees: [String: [Worktree]] = [:]
    /// Keyed by worktree path.
    private(set) var worktreeStatuses: [String: RepoStatus] = [:]
    /// Per repo: its default branch (`origin/main`), and the local branches already in it.
    private(set) var defaultBranches: [String: String] = [:]
    private(set) var mergedBranches: [String: Set<String>] = [:]
    /// Each repo's default branch's own line of commits: a branch pointing at one of them
    /// has no work of its own.
    private(set) var mainLines: [String: Set<String>] = [:]
    /// Detached worktrees, by path: whether some branch also holds their commit.
    private(set) var detachedReachable: [String: Bool] = [:]

    init(projectID: UUID, root: URL) {
        self.projectID = projectID
        self.root = root
    }

    var isRefreshing: Bool { isScanning || isRefreshingStatus || isRefreshingPulls }
    var repos: [WorkspaceRepo] { layout?.repos ?? [] }
    /// Linked worktrees inside the project are listed under their repo, not as repos.
    var clonedRepos: [WorkspaceRepo] {
        let linked = Set(worktrees.values.joined().map(\.path))
        return repos.filter { $0.isCloned && !linked.contains(Self.realPath($0.url.path)) }
    }
    var allWorktrees: [Worktree] { clonedRepos.flatMap { worktrees[$0.id] ?? [] } }

    /// The open PR from a worktree's branch.
    func pullRequest(for worktree: Worktree, in repo: WorkspaceRepo) -> PullRequest? {
        guard let branch = worktree.branch else { return nil }
        return pullRequests(for: repo).first { $0.headBranch == branch }
    }

    /// Done with: nothing uncommitted, no open PR, and its work has landed: its remote
    /// branch was deleted (what a merged PR usually does), or its own commits are in the
    /// default branch. A branch still where it started has no work yet, so it isn't done.
    func isFinished(_ worktree: Worktree, in repo: WorkspaceRepo) -> Bool {
        guard let branch = worktree.branch, !worktree.isMissing, pullRequest(for: worktree, in: repo) == nil,
              let status = worktreeStatuses[worktree.path], !status.isDirty
        else { return false }
        if status.upstreamGone { return true }
        return mergedBranches[repo.id]?.contains(branch) == true && !hasNoWork(worktree, in: repo)
    }

    /// What removing a worktree would cost: nothing (just do it), work that stays on its
    /// branch (ask), or work that's lost (ask, and say so).
    enum RemovalRisk: Equatable {
        case none
        case keepsWork(String)
        case losesWork(String)
    }

    func removalRisk(_ worktree: Worktree, in repo: WorkspaceRepo) -> RemovalRisk {
        if worktree.isMissing { return .none }
        guard let status = worktreeStatuses[worktree.path] else { return .keepsWork("Its state hasn't been read yet.") }
        if status.isDirty {
            let n = status.changedFiles
            return .losesWork("It has \(n) uncommitted change\(n == 1 ? "" : "s"), which will be lost.")
        }
        if worktree.isDetached {
            return detachedReachable[worktree.path] == true ? .none
                : .losesWork("It isn't on a branch, and its commits aren't on any branch either: they'd be lost. Make a branch in it first to keep them.")
        }
        if let pr = pullRequest(for: worktree, in: repo) {
            return .keepsWork("#\(pr.number) is still open. The branch \(worktree.branch ?? "") keeps its commits, so you can check it out again.")
        }
        if isFinished(worktree, in: repo) || hasNoWork(worktree, in: repo) { return .none }
        return .keepsWork("\(worktree.branch ?? "Its branch") has commits that aren't merged yet. The branch keeps them, so you can check it out again.")
    }

    /// Its branch points at a commit of the default branch's own: nothing done in it yet.
    func hasNoWork(_ worktree: Worktree, in repo: WorkspaceRepo) -> Bool {
        guard worktree.branch != nil, let head = worktree.headOID else { return false }
        return mainLines[repo.id]?.contains(head) == true
    }

    /// Which threads work in which checkout, by path: every folder a thread touched counts
    /// for the innermost checkout holding it, so a worktree nested in its repo's folder
    /// doesn't also count for the repo.
    func threadsByCheckout(_ threads: [ThreadModel]) -> [String: [ThreadModel]] {
        let roots = (clonedRepos.map { Self.realPath($0.url.path) } + allWorktrees.map(\.path)).sorted { $0.count > $1.count }
        var result: [String: [ThreadModel]] = [:]
        for thread in threads where !thread.isSideChat {
            var hit = Set<String>()
            for folder in thread.footprint + [thread.workingDirectory].compactMap({ $0.map(Self.realPath) }) {
                if let root = roots.first(where: { ThreadFootprint.touches([folder], root: $0) }) { hit.insert(root) }
            }
            for root in hit { result[root, default: []].append(thread) }
        }
        for key in result.keys { result[key]?.sort { $0.sortDate > $1.sortDate } }
        return result
    }

    /// A checkout a thread works in: a repo's main one (worktree nil) or a linked worktree.
    struct Checkout: Identifiable {
        let repo: WorkspaceRepo
        let worktree: Worktree?
        var id: String { worktree?.path ?? repo.id }
        var path: String { worktree?.path ?? WorkspaceModel.realPath(repo.url.path) }
    }

    /// For each thread that works here, the repos (main checkout paths) it works in.
    func checkoutsByThread(_ threads: [ThreadModel]) -> [UUID: [String]] {
        var result: [UUID: [String]] = [:]
        let repoOf = Dictionary(clonedRepos.flatMap { repo in
            [(Self.realPath(repo.url.path), repo)] + (worktrees[repo.id] ?? []).map { ($0.path, repo) }
        }, uniquingKeysWith: { first, _ in first })
        for (checkout, users) in threadsByCheckout(threads) {
            guard let repo = repoOf[checkout] else { continue }
            for thread in users { result[thread.id, default: []].append(Self.realPath(repo.url.path)) }
        }
        return result
    }

    /// The checkouts one thread works in, the one it runs in first.
    func checkouts(for thread: ThreadModel) -> [Checkout] {
        let all = clonedRepos.flatMap { repo in [Checkout(repo: repo, worktree: nil)] + (worktrees[repo.id] ?? []).map { Checkout(repo: repo, worktree: $0) } }
        let used = threadsByCheckout([thread])
        let running = thread.workingDirectory.map(Self.realPath)
        return all.filter { used[$0.path] != nil || $0.path == running }
            .sorted { ($0.path == running ? 0 : 1, $0.path) < ($1.path == running ? 0 : 1, $1.path) }
    }

    /// git reports real paths: `/tmp` is `/private/tmp`.
    nonisolated static func realPath(_ path: String) -> String {
        ThreadFootprint.resolve(path, in: "/") ?? path
    }
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
        let cloned = repos.filter(\.isCloned)
        let results = await Self.map(cloned) { repo in (repo.id, await git.status(of: repo.url)) }
        var statuses: [String: RepoStatus] = [:]
        for (id, status) in results { statuses[id] = status }
        if statuses != self.statuses { self.statuses = statuses }

        // Worktrees: each repo's list, the status of each one, and which branches are merged.
        let lists = await Self.map(cloned) { repo -> (String, [Worktree], String?) in
            let all = await git.worktrees(of: repo.url) ?? []
            return (repo.id, all.filter { !$0.isMain && !$0.isBare }, await git.defaultBranch(of: repo.url))
        }
        var worktrees: [String: [Worktree]] = [:], bases: [String: String] = [:]
        // A repo listed as a linked worktree of another (it's nested in the project) is
        // that one's worktree: only its main repo lists it.
        let mains = Set(lists.flatMap { _, linked, _ in linked.map(\.path) })
        for (id, linked, base) in lists {
            guard let repo = cloned.first(where: { $0.id == id }), !mains.contains(Self.realPath(repo.url.path)) else { continue }
            if !linked.isEmpty { worktrees[id] = linked.sorted { $0.path < $1.path } }
            if let base { bases[id] = base }
        }
        let merged = await Self.map(Array(bases)) { id, base in
            let url = cloned.first { $0.id == id }!.url
            return (id, await git.branchesMerged(into: base, in: url), await git.mainLine(base, in: url))
        }
        let checked = await Self.map(worktrees.values.joined().filter { !$0.isMissing }) { worktree in
            (worktree.path, await git.status(of: worktree.url))
        }
        var worktreeStatuses: [String: RepoStatus] = [:]
        for (path, status) in checked { worktreeStatuses[path] = status }
        let detached = worktrees.flatMap { id, list in list.filter { $0.isDetached && !$0.isMissing }.map { (id, $0) } }
        let reach = await Self.map(detached) { id, worktree in
            (worktree.path, await worktree.headOID.asyncMap { await git.isReachable($0, in: cloned.first { $0.id == id }!.url) } ?? false)
        }
        let reachable = Dictionary(reach, uniquingKeysWith: { $1 })
        if reachable != detachedReachable { detachedReachable = reachable }
        if worktrees != self.worktrees { self.worktrees = worktrees }
        if worktreeStatuses != self.worktreeStatuses { self.worktreeStatuses = worktreeStatuses }
        if bases != defaultBranches { defaultBranches = bases }
        let mergedMap = Dictionary(merged.map { ($0.0, $0.1) }, uniquingKeysWith: { $1 })
        if mergedMap != mergedBranches { mergedBranches = mergedMap }
        let lines = Dictionary(merged.map { ($0.0, $0.2) }, uniquingKeysWith: { $1 })
        if lines != mainLines { mainLines = lines }
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

extension Optional {
    func asyncMap<T>(_ transform: (Wrapped) async -> T) async -> T? {
        guard let value = self else { return nil }
        return await transform(value)
    }
}
