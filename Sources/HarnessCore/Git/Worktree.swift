import Foundation

/// One checkout of a repo: its main working tree, or a linked one (`git worktree add`).
public struct Worktree: Sendable, Hashable, Identifiable {
    public var path: String
    public var headOID: String?
    /// Short name; nil when HEAD is detached (or the repo is bare).
    public var branch: String?
    /// The repo's own checkout, the one the others hang off.
    public var isMain: Bool
    public var isBare: Bool
    /// `git worktree lock`: git won't prune or remove it. The reason, if one was given.
    public var lockReason: String?
    public var isLocked: Bool
    /// Its folder is gone; `git worktree prune` would drop it.
    public var isMissing: Bool

    public var id: String { path }
    public var url: URL { URL(fileURLWithPath: path, isDirectory: true) }
    public var isDetached: Bool { branch == nil && !isBare }

    public init(path: String, headOID: String? = nil, branch: String? = nil, isMain: Bool = false, isBare: Bool = false,
                lockReason: String? = nil, isLocked: Bool = false, isMissing: Bool = false) {
        self.path = path
        self.headOID = headOID
        self.branch = branch
        self.isMain = isMain
        self.isBare = isBare
        self.lockReason = lockReason
        self.isLocked = isLocked
        self.isMissing = isMissing
    }

    /// `git worktree list --porcelain`: blocks of `key value` lines, the main one first.
    public static func parse(porcelain output: String) -> [Worktree] {
        var worktrees: [Worktree] = []
        var current: Worktree?
        for line in output.split(separator: "\n", omittingEmptySubsequences: false) {
            let parts = line.split(separator: " ", maxSplits: 1).map(String.init)
            guard let key = parts.first, !key.isEmpty else {
                if let done = current { worktrees.append(done) }
                current = nil
                continue
            }
            let value = parts.count > 1 ? parts[1] : nil
            switch key {
            case "worktree":
                if let done = current { worktrees.append(done) }
                current = Worktree(path: value ?? "", isMain: worktrees.isEmpty)
            case "HEAD":
                current?.headOID = value
            case "branch":
                current?.branch = value.map { $0.hasPrefix("refs/heads/") ? String($0.dropFirst("refs/heads/".count)) : $0 }
            case "bare":
                current?.isBare = true
            case "locked":
                current?.isLocked = true
                current?.lockReason = value
            case "prunable":
                current?.isMissing = true
            default:
                break
            }
        }
        if let done = current { worktrees.append(done) }
        return worktrees
    }
}

extension Git {
    /// Every checkout of the repo at `repo`, the main one first.
    public func worktrees(of repo: URL) async -> [Worktree]? {
        let output = await run(["worktree", "list", "--porcelain"], in: repo)
        guard output.succeeded else { return nil }
        return Worktree.parse(porcelain: output.stdout)
    }

    /// The branch work lands in: the one `origin/HEAD` names (local `main` when you have
    /// it, else `origin/main`), or `main` or `master` without a remote.
    public func defaultBranch(of repo: URL) async -> String? {
        let remote = await run(["symbolic-ref", "--short", "refs/remotes/origin/HEAD"], in: repo)
        if remote.succeeded {
            let name = remote.stdout.trimmingCharacters(in: .whitespacesAndNewlines)
            let local = name.hasPrefix("origin/") ? String(name.dropFirst("origin/".count)) : name
            if !local.isEmpty, await branchExists(local, in: repo) { return local }
            if !name.isEmpty { return name }
        }
        for name in ["main", "master"] where await run(["rev-parse", "--verify", "--quiet", "refs/heads/\(name)"], in: repo).succeeded {
            return name
        }
        return nil
    }

    /// The commits on `base`'s own line (first parents only), newest first, up to `limit`.
    /// A branch pointing at one of these has no commits of its own; a merged branch's
    /// commits came in through a merge's second parent, so they aren't here.
    public func mainLine(_ base: String, in repo: URL, limit: Int = 5000) async -> Set<String> {
        let output = await run(["rev-list", "--first-parent", "--max-count=\(limit)", base], in: repo, outputLimit: 1024 * 1024)
        guard output.succeeded else { return [] }
        return Set(output.stdout.split(whereSeparator: \.isNewline).map(String.init))
    }

    /// Whether a branch, remote branch or tag holds `oid`: if not, a detached worktree's
    /// commits are only reachable from its HEAD, and removing it loses them.
    public func isReachable(_ oid: String, in repo: URL) async -> Bool {
        let output = await run(["for-each-ref", "--contains", oid, "--count=1", "--format=%(refname)",
                                "refs/heads", "refs/remotes", "refs/tags"], in: repo)
        return output.succeeded && !output.stdout.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    public func branchExists(_ branch: String, in repo: URL) async -> Bool {
        await run(["rev-parse", "--verify", "--quiet", "refs/heads/\(branch)"], in: repo).succeeded
    }

    /// Local branches whose tip is already in `base`: merged (by merge or fast-forward;
    /// a squash merge doesn't show here, a deleted remote branch usually does).
    public func branchesMerged(into base: String, in repo: URL) async -> Set<String> {
        let output = await run(["branch", "--merged", base, "--format=%(refname:short)"], in: repo)
        guard output.succeeded else { return [] }
        return Set(output.stdout.split(whereSeparator: \.isNewline).map(String.init))
    }

    /// `git worktree add`: a new checkout at `path`, on a new branch from `base`, or on an
    /// existing branch. Returns git's error when it fails.
    public func addWorktree(of repo: URL, at path: URL, newBranch: String?, from base: String?, existingBranch: String?) async -> String? {
        var arguments = ["worktree", "add"]
        if let newBranch { arguments += ["-b", newBranch] }
        arguments.append(path.path)
        if let existingBranch { arguments.append(existingBranch) } else if let base { arguments.append(base) }
        let output = await run(arguments, in: repo, writes: true)
        return output.succeeded ? nil : Self.message(output)
    }

    /// `git worktree remove`; `force` also drops uncommitted changes.
    public func removeWorktree(_ worktree: Worktree, of repo: URL, force: Bool) async -> String? {
        let output = await run(["worktree", "remove"] + (force ? ["--force"] : []) + [worktree.path], in: repo, writes: true)
        return output.succeeded ? nil : Self.message(output)
    }

    /// Forgets worktrees whose folders are gone.
    public func pruneWorktrees(of repo: URL) async -> String? {
        let output = await run(["worktree", "prune"], in: repo, writes: true)
        return output.succeeded ? nil : Self.message(output)
    }

    private static func message(_ output: CommandOutput) -> String {
        let text = output.stderr.trimmingCharacters(in: .whitespacesAndNewlines)
        return text.isEmpty ? "git exited with an error." : text.replacingOccurrences(of: "fatal: ", with: "")
    }
}
