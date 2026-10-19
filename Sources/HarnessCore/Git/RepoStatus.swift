import Foundation

/// A repo's branch and working-tree state, from `git status --porcelain=v2 --branch`.
public struct RepoStatus: Sendable, Equatable {
    /// Nil when HEAD is detached.
    public var branch: String?
    public var headOID: String?
    public var upstream: String?
    public var ahead = 0
    public var behind = 0
    public var staged = 0
    public var modified = 0
    public var untracked = 0
    public var conflicted = 0
    public var stashes = 0

    public init() {}

    public var changedFiles: Int { staged + modified + untracked + conflicted }
    public var isDirty: Bool { changedFiles > 0 }

    public static func parse(porcelainV2 output: String) -> RepoStatus {
        var status = RepoStatus()
        for line in output.split(separator: "\n", omittingEmptySubsequences: true) {
            if line.hasPrefix("# ") {
                let parts = line.dropFirst(2).split(separator: " ", maxSplits: 1).map(String.init)
                guard parts.count == 2 else { continue }
                switch parts[0] {
                case "branch.oid":
                    status.headOID = parts[1] == "(initial)" ? nil : parts[1]
                case "branch.head":
                    status.branch = parts[1] == "(detached)" ? nil : parts[1]
                case "branch.upstream":
                    status.upstream = parts[1]
                case "branch.ab":
                    for value in parts[1].split(separator: " ") {
                        if value.hasPrefix("+") { status.ahead = Int(value.dropFirst()) ?? 0 }
                        if value.hasPrefix("-") { status.behind = Int(value.dropFirst()) ?? 0 }
                    }
                case "stash":
                    status.stashes = Int(parts[1]) ?? 0
                default:
                    break
                }
                continue
            }
            switch line.first {
            case "1", "2":
                // "1 XY ..." where X is the index and Y the worktree; "." means unchanged.
                let xy = line.dropFirst(2).prefix(2)
                guard xy.count == 2 else { continue }
                if xy.first != "." { status.staged += 1 }
                if xy.last != "." { status.modified += 1 }
            case "u":
                status.conflicted += 1
            case "?":
                status.untracked += 1
            default:
                break
            }
        }
        return status
    }
}

/// Runs read-only git commands. `--no-optional-locks` keeps us from touching the index
/// while the user (or an agent) is working in the repo.
public struct Git: Sendable {
    public let executable: URL
    public let environment: [String: String]

    public init(executable: URL, environment: [String: String]) {
        self.executable = executable
        self.environment = environment
    }

    public func status(of repo: URL) async -> RepoStatus? {
        let output = await run(["status", "--porcelain=v2", "--branch", "--show-stash", "--untracked-files=normal"], in: repo)
        guard output.succeeded else { return nil }
        return RepoStatus.parse(porcelainV2: output.stdout)
    }

    public func originURL(of repo: URL) async -> String? {
        let output = await run(["config", "--get", "remote.origin.url"], in: repo)
        guard output.succeeded else { return nil }
        let url = output.stdout.trimmingCharacters(in: .whitespacesAndNewlines)
        return url.isEmpty ? nil : url
    }

    private func run(_ arguments: [String], in repo: URL) async -> CommandOutput {
        await CommandRunner.run(
            executable,
            arguments: ["--no-optional-locks", "-C", repo.path] + arguments,
            environment: environment,
            timeout: 15
        )
    }
}
