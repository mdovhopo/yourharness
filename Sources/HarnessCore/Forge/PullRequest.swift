import Foundation

/// A pull request (GitHub) or merge request (GitLab), normalized. The repos view lists open
/// ones; a thread can link to merged or closed ones too.
public struct PullRequest: Sendable, Hashable, Identifiable {
    public enum State: String, Sendable, Hashable, Codable {
        case open, merged, closed
    }

    public enum Review: String, Sendable, Hashable {
        case approved, changesRequested, required, none
    }

    public enum Checks: String, Sendable, Hashable {
        case passing, failing, pending, none
    }

    public var remote: GitRemote
    public var number: Int
    public var title: String
    public var url: URL
    public var author: String?
    public var isDraft: Bool
    public var headBranch: String
    public var baseBranch: String
    public var updatedAt: Date?
    public var review: Review
    public var checks: Checks
    public var isMine: Bool
    public var reviewRequestedFromMe: Bool
    public var state: State = .open
    /// What GitHub says stands between it and merging: "CLEAN", "BLOCKED", "BEHIND",
    /// "DIRTY" (conflicts), "UNSTABLE" (failing checks that aren't required), "DRAFT".
    public var mergeStatus: String?
    /// Auto-merge is on: it merges by itself once checks and reviews allow.
    public var autoMerge = false
    public var inMergeQueue = false
    public var mergedAt: Date?
    /// Its place in one of GitHub's own stacks (`gh stack`), when it's in one.
    public var stack: StackPlace?

    /// Which of a repo's GitHub stacks a PR is in, how many PRs it has, and where this
    /// one sits: 1 is the bottom, on the stack's base branch.
    public struct StackPlace: Sendable, Hashable {
        public var number: Int
        public var size: Int
        public var position: Int

        public init(number: Int, size: Int, position: Int) {
            self.number = number
            self.size = size
            self.position = position
        }
    }

    public var id: String { "\(remote.host)/\(remote.path)#\(number)" }

    public init(
        remote: GitRemote, number: Int, title: String, url: URL, author: String?, isDraft: Bool,
        headBranch: String, baseBranch: String, updatedAt: Date?, review: Review, checks: Checks,
        isMine: Bool, reviewRequestedFromMe: Bool, state: State = .open
    ) {
        self.state = state
        self.remote = remote
        self.number = number
        self.title = title
        self.url = url
        self.author = author
        self.isDraft = isDraft
        self.headBranch = headBranch
        self.baseBranch = baseBranch
        self.updatedAt = updatedAt
        self.review = review
        self.checks = checks
        self.isMine = isMine
        self.reviewRequestedFromMe = reviewRequestedFromMe
    }
}

/// Everything one forge reported in a refresh.
public struct ForgeSnapshot: Sendable, Equatable {
    public var forge: Forge
    /// The signed-in user, per host.
    public var accounts: [String: String] = [:]
    public var pullRequests: [PullRequest] = []
    /// Repos the forge couldn't resolve (no access, renamed, deleted).
    public var unreachable: Set<GitRemote> = []
    /// Set when the whole request failed: CLI missing, signed out, offline.
    public var error: String?

    public init(forge: Forge) {
        self.forge = forge
    }
}

enum ForgeDates {
    nonisolated(unsafe) static let formatter: ISO8601DateFormatter = {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return formatter
    }()

    nonisolated(unsafe) static let plainFormatter = ISO8601DateFormatter()

    static func parse(_ value: Any?) -> Date? {
        guard let text = value as? String else { return nil }
        return plainFormatter.date(from: text) ?? formatter.date(from: text)
    }
}

public extension Forge {
    /// The official CLI we drive for this forge.
    var cliName: String {
        switch self {
        case .github: "gh"
        case .gitlab: "glab"
        }
    }

    var displayName: String {
        switch self {
        case .github: "GitHub"
        case .gitlab: "GitLab"
        }
    }

    var installURL: URL {
        switch self {
        case .github: URL(string: "https://cli.github.com")!
        case .gitlab: URL(string: "https://gitlab.com/gitlab-org/cli")!
        }
    }

    /// What the forge calls a pull request.
    var requestNoun: String {
        switch self {
        case .github: "pull request"
        case .gitlab: "merge request"
        }
    }
}
